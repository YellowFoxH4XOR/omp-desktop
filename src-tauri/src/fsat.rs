//! Descriptor-relative file operations for repository files.
//!
//! Each function takes a directory that is already open and one name inside
//! it (a single path component), and never follows a symlink at that name.
//! Walking a path one component at a time through these keeps a swapped-in
//! symlink from redirecting a write outside the repository.

use std::ffi::CStr;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

const DIRECTORY: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;

fn owned(fd: RawFd) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by a successful open and has no other owner.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn done(status: libc::c_int) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Open a directory by path; fails if the final component is a symlink.
pub fn open_dir(path: &CStr) -> io::Result<File> {
    // SAFETY: `path` is a valid NUL-terminated string for the whole call.
    owned(unsafe { libc::open(path.as_ptr(), DIRECTORY) })
}

pub fn open_dir_at(dir: &File, name: &CStr) -> io::Result<File> {
    // SAFETY: `dir` is an open descriptor and `name` a valid C string.
    owned(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), DIRECTORY) })
}

/// Open an existing entry for reading.
pub fn open_file_at(dir: &File, name: &CStr) -> io::Result<File> {
    // SAFETY: `dir` is an open descriptor and `name` a valid C string.
    owned(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW,
        )
    })
}

/// Create a file for writing; fails if the name already exists. `mode` is
/// still subject to the process umask.
pub fn create_new_at(dir: &File, name: &CStr, mode: u32) -> io::Result<File> {
    // SAFETY: `dir` is an open descriptor and `name` a valid C string.
    owned(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            mode as libc::c_uint,
        )
    })
}

pub fn mkdir_at(dir: &File, name: &CStr, mode: libc::mode_t) -> io::Result<()> {
    // SAFETY: `dir` is an open descriptor and `name` a valid C string.
    done(unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), mode) })
}

/// Best-effort cleanup; a failure leaves only a stray temporary file.
pub fn unlink_at(dir: &File, name: &CStr) {
    // SAFETY: `dir` is an open descriptor and `name` a valid C string.
    unsafe {
        libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0);
    }
}

/// Atomically move `from` to `to`, failing if `to` already exists.
#[cfg(target_os = "macos")]
pub fn rename_no_replace(dir: &File, from: &CStr, to: &CStr) -> io::Result<()> {
    rename_with(dir, from, to, libc::RENAME_EXCL)
}

/// Atomically exchange two entries.
#[cfg(target_os = "macos")]
pub fn rename_swap(dir: &File, a: &CStr, b: &CStr) -> io::Result<()> {
    rename_with(dir, a, b, libc::RENAME_SWAP)
}

#[cfg(target_os = "macos")]
fn rename_with(dir: &File, from: &CStr, to: &CStr, flags: libc::c_uint) -> io::Result<()> {
    let fd = dir.as_raw_fd();
    // SAFETY: `dir` is an open descriptor and both names are valid C strings.
    done(unsafe { libc::renameatx_np(fd, from.as_ptr(), fd, to.as_ptr(), flags) })
}

/// Atomically move `from` to `to`, failing if `to` already exists.
#[cfg(target_os = "linux")]
pub fn rename_no_replace(dir: &File, from: &CStr, to: &CStr) -> io::Result<()> {
    rename_with(dir, from, to, libc::RENAME_NOREPLACE)
}

/// Atomically exchange two entries.
#[cfg(target_os = "linux")]
pub fn rename_swap(dir: &File, a: &CStr, b: &CStr) -> io::Result<()> {
    rename_with(dir, a, b, libc::RENAME_EXCHANGE)
}

#[cfg(target_os = "linux")]
fn rename_with(dir: &File, from: &CStr, to: &CStr, flags: libc::c_uint) -> io::Result<()> {
    let fd = dir.as_raw_fd();
    // SAFETY: `dir` is an open descriptor and both names are valid C strings.
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            fd,
            from.as_ptr(),
            fd,
            to.as_ptr(),
            flags,
        )
    };
    done(status as libc::c_int)
}

/// POSIX rename has no portable no-replace form. Linking the inode into
/// place is atomic and fails if the name appeared in the meantime.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn rename_no_replace(dir: &File, from: &CStr, to: &CStr) -> io::Result<()> {
    let fd = dir.as_raw_fd();
    // SAFETY: `dir` is an open descriptor and both names are valid C strings.
    let linked = done(unsafe { libc::linkat(fd, from.as_ptr(), fd, to.as_ptr(), 0) });
    unlink_at(dir, from);
    linked
}

/// Atomically move `from` over `to`, replacing it.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn rename_replace(dir: &File, from: &CStr, to: &CStr) -> io::Result<()> {
    let fd = dir.as_raw_fd();
    // SAFETY: `dir` is an open descriptor and both names are valid C strings.
    done(unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) })
}
