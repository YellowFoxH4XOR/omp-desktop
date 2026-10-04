//! Writing a reviewed file: every path component is opened without
//! following symlinks, and the replacement is swapped in only while the
//! file still has the content the user reviewed.

use super::*;

#[cfg(unix)]
pub(super) fn c_path(path: &Path) -> AppResult<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| AppError::new("Path contains a NUL byte."))
}

#[cfg(unix)]
pub(super) fn open_checked_dir(root: &Path, path: &Path) -> AppResult<std::fs::File> {
    open_checked_dir_impl(root, path, true)
}

#[cfg(unix)]
pub(super) fn open_checked_dir_no_create(root: &Path, path: &Path) -> AppResult<std::fs::File> {
    open_checked_dir_impl(root, path, false)
}

#[cfg(unix)]
pub(super) fn open_checked_dir_impl(
    root: &Path,
    path: &Path,
    create: bool,
) -> AppResult<std::fs::File> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AppError::new("Path is outside the repository."))?;
    let mut current = fsat::open_dir(&c_path(root)?)
        .map_err(|error| AppError::new(format!("Could not open repository root: {error}")))?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(AppError::new("Path is outside the repository."));
        };
        let name = c_path(Path::new(component))?;
        let next = match fsat::open_dir_at(&current, &name) {
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                fsat::mkdir_at(&current, &name, 0o755).map_err(|error| {
                    AppError::new(format!("Could not create parent directory: {error}"))
                })?;
                fsat::open_dir_at(&current, &name)
            }
            result => result,
        };
        current = next.map_err(|error| {
            AppError::new(format!("Could not safely open file parent: {error}"))
        })?;
    }
    Ok(current)
}

#[cfg(unix)]
pub(super) fn open_checked_file(root: &Path, abs: &Path) -> AppResult<std::fs::File> {
    open_checked_file_impl(root, abs, true)
}

#[cfg(unix)]
pub(super) fn open_checked_file_no_create(root: &Path, abs: &Path) -> AppResult<std::fs::File> {
    open_checked_file_impl(root, abs, false)
}

#[cfg(unix)]
pub(super) fn open_checked_file_impl(
    root: &Path,
    abs: &Path,
    create: bool,
) -> AppResult<std::fs::File> {
    let parent = abs
        .parent()
        .ok_or_else(|| AppError::new("Path is outside the repository."))?;
    let parent_dir = if create {
        open_checked_dir(root, parent)?
    } else {
        open_checked_dir_no_create(root, parent)?
    };
    let name = abs
        .file_name()
        .ok_or_else(|| AppError::new("Path must name a file."))?;
    let file = fsat::open_file_at(&parent_dir, &c_path(Path::new(name))?)
        .map_err(|error| AppError::new(format!("Could not safely open file: {error}")))?;
    parent_dir.metadata()?;
    Ok(file)
}

#[cfg(unix)]
pub(super) fn write_checked_unix(
    root: &Path,
    abs: &Path,
    expected_hash: &str,
    content: &str,
) -> AppResult<()> {
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::fs::PermissionsExt;
    let parent = abs
        .parent()
        .ok_or_else(|| AppError::new("Path is outside the repository."))?;
    let parent_dir = open_checked_dir(root, parent)?;
    let name = abs
        .file_name()
        .ok_or_else(|| AppError::new("Path must name a file."))?;
    let current = match open_checked_file(root, abs) {
        Ok(file) => Some(file),
        Err(error) => match std::fs::symlink_metadata(abs) {
            Ok(_) => return Err(error),
            Err(meta_error) if meta_error.kind() == std::io::ErrorKind::NotFound => None,
            Err(meta_error) => {
                return Err(AppError::new(format!(
                    "Could not inspect file: {meta_error}"
                )))
            }
        },
    };
    let current_was_present = current.is_some();
    let (current_hash, mode) = if let Some(mut file) = current {
        let metadata = file
            .metadata()
            .map_err(|e| AppError::new(format!("Could not inspect file: {e}")))?;
        if !metadata.is_file() {
            return Err(AppError::new("Only regular files support hunk edits."));
        }
        let hash = hash_reader(root, &mut file)?;
        (hash, metadata.permissions().mode() & 0o7777)
    } else {
        (hash_bytes(root, b"")?, 0o644)
    };
    if current_hash != expected_hash {
        return Err(AppError::new(
            "The file changed on disk since you loaded it. Refresh the diff and try again.",
        ));
    }

    let temp_name = format!(
        ".pidesk-review-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    let temp_c = c_path(Path::new(&temp_name))?;
    let mut temp = fsat::create_new_at(&parent_dir, &temp_c, mode)
        .map_err(|error| AppError::new(format!("Could not create temporary file: {error}")))?;
    let mut write_result = temp
        .write_all(content.as_bytes())
        .and_then(|_| temp.sync_all());
    if current_was_present && write_result.is_ok() {
        // The umask narrowed the mode at creation; an edited file keeps the
        // permissions it had.
        write_result = temp.set_permissions(std::fs::Permissions::from_mode(mode));
    }
    if let Err(error) = write_result {
        fsat::unlink_at(&parent_dir, &temp_c);
        return Err(AppError::new(format!("Could not write file: {error}")));
    }

    if current_was_present {
        let mut original = match open_checked_file(root, abs) {
            Ok(file) => file,
            Err(error) => {
                fsat::unlink_at(&parent_dir, &temp_c);
                return Err(error);
            }
        };
        if let Err(error) = original.seek(SeekFrom::Start(0)) {
            fsat::unlink_at(&parent_dir, &temp_c);
            return Err(AppError::new(format!("Could not recheck file: {error}")));
        }
        let after = match hash_reader(root, &mut original) {
            Ok(hash) => hash,
            Err(error) => {
                fsat::unlink_at(&parent_dir, &temp_c);
                return Err(error);
            }
        };
        if after != expected_hash {
            fsat::unlink_at(&parent_dir, &temp_c);
            return Err(AppError::new(
                "The file changed on disk while saving. Refresh the diff and try again.",
            ));
        }
    }

    // The original descriptor was consumed by the hash above; retain whether
    // the path existed so the replacement can use create-vs-swap semantics.
    atomic_replace(
        &parent_dir,
        &temp_c,
        &c_path(Path::new(name))?,
        root,
        abs,
        expected_hash,
        current_was_present,
    )
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn atomic_replace(
    parent: &std::fs::File,
    temp: &std::ffi::CString,
    name: &std::ffi::CString,
    root: &Path,
    abs: &Path,
    expected_hash: &str,
    current_was_present: bool,
) -> AppResult<()> {
    if !current_was_present {
        // A plain rename would replace a destination created after the
        // initial probe; the no-replace form preserves that concurrent work.
        return fsat::rename_no_replace(parent, temp, name).map_err(|error| {
            fsat::unlink_at(parent, temp);
            AppError::new(format!(
                "Could not atomically create file without replacing concurrent work: {error}"
            ))
        });
    }
    // Verified exchange: swap the new bytes into place, hash what was
    // displaced, and swap back on a mismatch so a concurrent edit is never
    // silently overwritten.
    if let Err(error) = fsat::rename_swap(parent, temp, name) {
        fsat::unlink_at(parent, temp);
        return Err(AppError::new(format!(
            "Could not atomically replace file: {error}"
        )));
    }
    let displaced = hash_file(root, &abs.with_file_name(temp.to_string_lossy().as_ref()));
    if matches!(&displaced, Ok(hash) if hash == expected_hash) {
        fsat::unlink_at(parent, temp);
        return Ok(());
    }
    if fsat::rename_swap(parent, temp, name).is_err() {
        // The displaced bytes stay in the temporary file rather than being lost.
        return Err(AppError::new(match displaced {
            Ok(_) => "Concurrent edit detected; rollback failed.",
            Err(_) => "Could not verify swapped file; rollback failed.",
        }));
    }
    fsat::unlink_at(parent, temp);
    Err(displaced.err().unwrap_or_else(|| {
        AppError::new("The file changed on disk while saving. Refresh the diff and try again.")
    }))
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "linux")))]
pub(super) fn atomic_replace(
    parent: &std::fs::File,
    temp: &std::ffi::CString,
    name: &std::ffi::CString,
    _root: &Path,
    _abs: &Path,
    _expected_hash: &str,
    current_was_present: bool,
) -> AppResult<()> {
    if !current_was_present {
        return fsat::rename_no_replace(parent, temp, name).map_err(|error| {
            AppError::new(format!(
                "Could not atomically create file without replacing concurrent work: {error}"
            ))
        });
    }
    fsat::rename_replace(parent, temp, name).map_err(|error| {
        fsat::unlink_at(parent, temp);
        AppError::new(format!("Could not atomically replace file: {error}"))
    })
}

#[cfg(not(unix))]
pub(super) fn write_checked_unix(
    _root: &Path,
    abs: &Path,
    expected_hash: &str,
    content: &str,
) -> AppResult<()> {
    let current = hash_file(Path::new("."), abs)?;
    if current != expected_hash {
        return Err(AppError::new(
            "The file changed on disk since you loaded it.",
        ));
    }
    std::fs::write(abs, content).map_err(|e| AppError::new(format!("Could not write file: {e}")))
}
