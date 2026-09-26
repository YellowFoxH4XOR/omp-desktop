use crate::error::{AppError, AppResult};
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

pub const MAX_TEXT: usize = 64 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    PrivatePi,
    Project,
    PiDocs,
}

fn protected(path: &Path) -> bool {
    [
        util::home_dir().join(".pi"),
        util::home_dir().join(".ssh"),
        util::home_dir().join(".aws"),
        util::home_dir().join(".azure"),
        util::home_dir().join(".config/gcloud"),
        util::home_dir().join(".netrc"),
        util::home_dir().join(".npmrc"),
        util::home_dir().join(".git-credentials"),
        util::home_dir().join("Library"),
        util::pidesk_root().join("intern"),
        util::agent_dir().join("auth.json"),
        util::agent_dir().join("models.json"),
        util::agent_dir().join("sessions"),
    ]
    .iter()
    .any(|base| path.starts_with(util::resolve_path(base)))
}

pub fn project_root(path: &Path) -> AppResult<PathBuf> {
    let path = path.canonicalize()?;
    let home = util::resolve_path(&util::home_dir());
    let hidden_home_root = path
        .strip_prefix(&home)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|part| part.as_os_str().to_string_lossy().starts_with('.'))
        && !path.starts_with(util::resolve_path(&util::pidesk_root().join("worktrees")));
    if home.starts_with(&path) || protected(&path) || hidden_home_root {
        return Err(AppError::new("Pi Intern requires a dedicated project directory, not the home directory, an ancestor of it, or credential/application-data folders."));
    }
    Ok(path)
}

pub struct BoundDirectory {
    source: PathBuf,
    path: PathBuf,
    handle: Option<File>,
}
impl BoundDirectory {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn capture(path: &Path) -> AppResult<Self> {
        let source = util::normalize_path(path);
        let path = util::resolve_path(path);
        let handle = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            source,
            path,
            handle,
        })
    }
    pub fn same_directory(&self, file: &File) -> AppResult<bool> {
        Ok(self.handle.as_ref().is_some_and(|approved| {
            match (approved.metadata(), file.metadata()) {
                (Ok(a), Ok(b)) => Identity::of(&a) == Identity::of(&b),
                _ => false,
            }
        }))
    }
    /// Bind the child's working directory to the actual approved descriptor.
    /// The last path identity check happens before exec, but fchdir ensures a
    /// swap even after that check cannot redirect Git or Pi to a replacement.
    pub fn configure_command(&self, command: &mut std::process::Command) -> AppResult<File> {
        use std::os::unix::process::CommandExt;
        self.verify()?;
        let file = self
            .handle
            .as_ref()
            .ok_or_else(|| AppError::new("The approved directory is missing."))?
            .try_clone()?;
        let child_file = file.try_clone()?;
        let source = c_name(self.source.as_os_str())?;
        let expected = Identity::of(&file.metadata()?);
        unsafe {
            command.pre_exec(move || {
                let mut meta: libc::stat = std::mem::zeroed();
                if libc::stat(source.as_ptr(), &mut meta) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if meta.st_dev as u64 != expected.device || meta.st_ino != expected.inode {
                    return Err(std::io::Error::from_raw_os_error(libc::ESTALE));
                }
                if libc::fchdir(child_file.as_raw_fd()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(file)
    }

    pub fn verify(&self) -> AppResult<()> {
        if util::resolve_path(&self.source) != self.path {
            return Err(AppError::new(
                "The approved directory path now points elsewhere. Request a fresh plan.",
            ));
        }
        match (&self.handle, std::fs::symlink_metadata(&self.path)) {
            (Some(file), Ok(meta))
                if meta.is_dir()
                    && !meta.file_type().is_symlink()
                    && Identity::of(&meta) == Identity::of(&file.metadata()?) =>
            {
                Ok(())
            }
            (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(AppError::new(
                "An approved project or worktree was moved or replaced. Request a fresh plan.",
            )),
        }
    }
}

pub fn resolve(base: &Path, relative: &str, scope: Scope, write: bool) -> AppResult<PathBuf> {
    if relative.len() > 4096 || relative.contains('\0') {
        return Err(AppError::new("Invalid path."));
    }
    let mut path = if scope == Scope::Project {
        project_root(base)?
    } else {
        base.canonicalize()?
    };
    for part in Path::new(relative).components() {
        let Component::Normal(name) = part else {
            return Err(AppError::new(
                "Use a relative path without parent traversal.",
            ));
        };
        path.push(name);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AppError::new("Pi Intern does not follow symbolic links."))
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    if protected(&path) {
        return Err(AppError::new("Pi Intern cannot access credentials, session storage, its own controls, or terminal Pi."));
    }
    if scope == Scope::PiDocs && write {
        return Err(AppError::new("Installed Pi documentation is read-only."));
    }
    if scope == Scope::PrivatePi {
        let files = [
            "agent/settings.json",
            "agent/keybindings.json",
            "agent/AGENTS.md",
            "agent/APPEND_SYSTEM.md",
            "agent/SYSTEM.md",
        ];
        let directories = [
            "agent/extensions",
            "agent/skills",
            "agent/prompts",
            "agent/themes",
        ];
        if !files.contains(&relative)
            && !directories
                .iter()
                .any(|entry| Path::new(relative).starts_with(entry))
            && !(relative.is_empty() && !write)
        {
            return Err(AppError::new("Use a private Pi configuration or resource path. Credentials and models.json are excluded."));
        }
    }
    Ok(path)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            device: meta.dev(),
            inode: meta.ino(),
        }
    }
}

/// An approval retains directory handles/identities, not merely mutable path
/// strings. openat walks from the captured root, and renameat never follows a
/// replaced path component. Renamed/replaced targets require a fresh plan.
pub struct BoundPath {
    pub path: PathBuf,
    root: File,
    root_path: PathBuf,
    relative: PathBuf,
    directories: Vec<(PathBuf, Identity)>,
    leaf: Option<Identity>,
    mode: libc::mode_t,
}
impl BoundPath {
    pub fn capture(base: &Path, relative: &str, scope: Scope, write: bool) -> AppResult<Self> {
        let path = resolve(base, relative, scope, write)?;
        let root_path = base.canonicalize()?;
        let relative = path
            .strip_prefix(&root_path)
            .map_err(|_| AppError::new("Invalid target."))?
            .to_path_buf();
        if relative.as_os_str().is_empty() {
            return Err(AppError::new("A file path is required."));
        }
        let root = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&root_path)?;
        let mut directories = vec![(root_path.clone(), Identity::of(&root.metadata()?))];
        let mut parent = root_path.clone();
        for part in relative.parent().unwrap_or(Path::new("")).components() {
            parent.push(part);
            match std::fs::symlink_metadata(&parent) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                    directories.push((parent.clone(), Identity::of(&meta)))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                _ => return Err(AppError::new("Target parent is not a real directory.")),
            }
        }
        let leaf = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                Some(Identity::of(&meta))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            _ => return Err(AppError::new("Only regular files are supported.")),
        };
        let mode = std::fs::symlink_metadata(&path)
            .map(|meta| (meta.mode() & 0o777) as libc::mode_t)
            .unwrap_or(0o600);
        let bound = Self {
            path,
            root,
            root_path,
            relative,
            directories,
            leaf,
            mode,
        };
        bound.verify()?;
        Ok(bound)
    }
    pub fn verify(&self) -> AppResult<()> {
        for (path, identity) in &self.directories {
            let meta = std::fs::symlink_metadata(path)?;
            if meta.file_type().is_symlink() || !meta.is_dir() || Identity::of(&meta) != *identity {
                return Err(AppError::new(
                    "An approved directory was moved or replaced. Request a new plan.",
                ));
            }
        }
        let now = match std::fs::symlink_metadata(&self.path) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                Some(Identity::of(&meta))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            _ => {
                return Err(AppError::new(
                    "The approved file was replaced. Request a new plan.",
                ))
            }
        };
        if now != self.leaf {
            return Err(AppError::new(
                "The approved file was moved or replaced. Request a new plan.",
            ));
        }
        Ok(())
    }
    fn parent(&self, create: bool) -> AppResult<File> {
        let mut current = self.root.try_clone()?;
        let mut path = self.root_path.clone();
        for part in self.relative.parent().unwrap_or(Path::new("")).components() {
            let Component::Normal(name) = part else {
                return Err(AppError::new("Invalid parent."));
            };
            path.push(name);
            let name = c_name(name)?;
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            let mut fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0
                && create
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT)
            {
                if unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o755) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
            }
            if fd < 0 {
                return Err(AppError::new("Could not safely open target parent."));
            }
            current = unsafe { File::from_raw_fd(fd) };
            if let Some((_, identity)) = self
                .directories
                .iter()
                .find(|(captured, _)| *captured == path)
            {
                if Identity::of(&current.metadata()?) != *identity {
                    return Err(AppError::new(
                        "An approved parent directory changed. Request a new plan.",
                    ));
                }
            }
        }
        Ok(current)
    }
}
fn c_name(name: &std::ffi::OsStr) -> AppResult<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(name.as_bytes()).map_err(|_| AppError::new("Invalid filename."))
}
fn read_at(parent: &File, name: &std::ffi::CString) -> AppResult<Option<String>> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(AppError::new("Could not safely read target file."));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_TEXT as u64 {
        return Err(AppError::new(
            "Only regular text files up to 64 KiB are supported.",
        ));
    }
    let mut text = String::new();
    file.take(MAX_TEXT as u64 + 1).read_to_string(&mut text)?;
    if text.len() > MAX_TEXT {
        return Err(AppError::new("File grew past the Intern text limit."));
    }
    Ok(Some(text))
}

pub fn read(base: &Path, path: &str, scope: Scope) -> AppResult<Value> {
    let bound = BoundPath::capture(base, path, scope, false)?;
    if bound.leaf.is_none() {
        return Ok(json!({"content":null}));
    }
    let parent = bound.parent(false)?;
    Ok(json!({"content":read_at(&parent,&c_name(bound.path.file_name().unwrap())?)?}))
}
pub fn list(base: &Path, path: &str, scope: Scope) -> AppResult<Value> {
    let path = resolve(base, path, scope, false)?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)?.take(256) {
        let entry = entry?;
        entries.push(json!({"name":entry.file_name().to_string_lossy(),"directory":entry.file_type()?.is_dir(),"symlink":entry.file_type()?.is_symlink()}));
    }
    Ok(json!({"entries":entries,"limit":256}))
}

pub fn write_bound(
    bound: &BoundPath,
    expected: &Option<String>,
    content: &str,
) -> AppResult<Value> {
    write_bound_inner(bound, expected, content, || {})
}

fn write_bound_inner(
    bound: &BoundPath,
    expected: &Option<String>,
    content: &str,
    before_swap: impl FnOnce(),
) -> AppResult<Value> {
    if content.len() > MAX_TEXT || expected.as_ref().is_some_and(|text| text.len() > MAX_TEXT) {
        return Err(AppError::new("File exceeds the Intern text limit."));
    }
    bound.verify()?;
    let parent = bound.parent(true)?;
    let name = c_name(bound.path.file_name().unwrap())?;
    if read_at(&parent, &name)? != *expected {
        return Err(AppError::new("The file changed since this plan was proposed. Read it again and request fresh approval."));
    }
    let temp = c_name(std::ffi::OsStr::new(&format!(
        ".pidesk-intern-{}",
        uuid::Uuid::new_v4()
    )))?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            temp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut retain_temp = false;
    let result = (|| -> AppResult<()> {
        let mut file = unsafe { File::from_raw_fd(fd) };
        if unsafe { libc::fchmod(file.as_raw_fd(), bound.mode) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        before_swap();
        #[cfg(target_os = "macos")]
        {
            let flags = if expected.is_some() {
                libc::RENAME_SWAP
            } else {
                libc::RENAME_EXCL
            };
            if unsafe {
                libc::renameatx_np(
                    parent.as_raw_fd(),
                    temp.as_ptr(),
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    flags,
                )
            } != 0
            {
                return Err(AppError::new(
                    "Target changed during approval. No file was overwritten.",
                ));
            }
            let mut replaced: libc::stat = unsafe { std::mem::zeroed() };
            let identity_matches = unsafe {
                libc::fstatat(
                    parent.as_raw_fd(),
                    temp.as_ptr(),
                    &mut replaced,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } == 0
                && bound.leaf
                    == Some(Identity {
                        device: replaced.st_dev as u64,
                        inode: replaced.st_ino,
                    })
                && replaced.st_mode & 0o777 == bound.mode;
            if expected.is_some()
                && (!identity_matches
                    || !matches!(read_at(&parent,&temp),Ok(ref old) if old==expected))
            {
                if unsafe {
                    libc::renameatx_np(
                        parent.as_raw_fd(),
                        temp.as_ptr(),
                        parent.as_raw_fd(),
                        name.as_ptr(),
                        libc::RENAME_SWAP,
                    )
                } != 0
                {
                    retain_temp = true;
                    return Err(AppError::new("Concurrent edit detected; rollback failed. Original content is retained in the temporary file."));
                }
                return Err(AppError::new(
                    "Concurrent edit preserved. Request a fresh plan.",
                ));
            }
        }
        #[cfg(not(target_os = "macos"))]
        return Err(AppError::new(
            "Intern file writes require macOS atomic replacement.",
        ));
        Ok(())
    })();
    if !retain_temp {
        unsafe {
            libc::unlinkat(parent.as_raw_fd(), temp.as_ptr(), 0);
        }
    }
    result?;
    Ok(json!({"written":bound.path}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_processes_cannot_reenter_a_replaced_directory() {
        let root =
            std::env::temp_dir().join(format!("pidesk-intern-preexec-{}", uuid::Uuid::new_v4()));
        for git in [false, true] {
            let path = root.join(if git { "repo" } else { "thread" });
            std::fs::create_dir_all(&path).unwrap();
            let bound = BoundDirectory::capture(&path).unwrap();
            let mut command =
                std::process::Command::new(if git { "/usr/bin/git" } else { "/bin/sh" });
            command
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .current_dir(&path);
            if git {
                command.args(["-C", ".", "init", "-q"]);
            } else {
                command.args(["-c", "printf bad > prompted"]);
            }
            let _capability = bound.configure_command(&mut command).unwrap();
            let old = path.with_extension("old");
            std::fs::rename(&path, &old).unwrap();
            std::fs::create_dir(&path).unwrap();
            assert!(command.output().is_err());
            for directory in [&path, &old] {
                assert!(!directory.join("prompted").exists());
                assert!(!directory.join(".git").exists());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn broad_roots_and_known_secrets_are_never_project_capabilities() {
        assert!(project_root(&util::home_dir()).is_err());
        assert!(project_root(Path::new("/")).is_err());
        assert!(protected(&util::agent_dir().join("models.json")));
        assert!(protected(&util::home_dir().join(".ssh/id_rsa")));
        assert!(resolve(
            &util::home_dir(),
            ".pidesk/agent/models.json",
            Scope::Project,
            false
        )
        .is_err());
    }

    #[test]
    fn identical_text_replacement_cannot_change_approved_identity_or_mode() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("pidesk-intern-leaf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("script.sh");
        std::fs::write(&path, "original").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let bound = BoundPath::capture(&root, "script.sh", Scope::Project, true).unwrap();
        let result = write_bound_inner(&bound, &Some("original".into()), "changed", || {
            std::fs::rename(&path, root.join("old")).unwrap();
            std::fs::write(&path, "original").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o755);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_binding_detects_replaced_or_new_worktrees() {
        let root =
            std::env::temp_dir().join(format!("pidesk-intern-directory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("worktree")).unwrap();
        let bound = BoundDirectory::capture(&root.join("worktree")).unwrap();
        std::fs::rename(root.join("worktree"), root.join("old")).unwrap();
        std::fs::create_dir(root.join("worktree")).unwrap();
        assert!(bound.verify().is_err());
        let absent = BoundDirectory::capture(&root.join("absent")).unwrap();
        std::fs::create_dir(root.join("absent")).unwrap();
        assert!(absent.verify().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paths_and_expected_content_fail_closed() {
        let root =
            std::env::temp_dir().join(format!("pidesk-intern-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        assert!(resolve(&root, "../outside", Scope::Project, true).is_err());
        let bound = BoundPath::capture(&root, "test.txt", Scope::Project, true).unwrap();
        write_bound(&bound, &None, "first").unwrap();
        assert!(write_bound(&bound, &None, "clobber").is_err());
        let bound = BoundPath::capture(&root, "test.txt", Scope::Project, true).unwrap();
        assert!(write_bound(&bound, &Some("stale".into()), "clobber").is_err());
        write_bound(&bound, &Some("first".into()), "second").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("test.txt")).unwrap(),
            "second"
        );
        std::os::unix::fs::symlink(root.join("test.txt"), root.join("link")).unwrap();
        assert!(resolve(&root, "link", Scope::Project, true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn root_and_parent_replacements_revoke_approval() {
        let root =
            std::env::temp_dir().join(format!("pidesk-intern-replace-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("parent")).unwrap();
        let bound = BoundPath::capture(&root, "parent/file", Scope::Project, true).unwrap();
        std::fs::rename(root.join("parent"), root.join("old")).unwrap();
        std::fs::create_dir(root.join("parent")).unwrap();
        assert!(write_bound(&bound, &None, "bad").is_err());
        assert!(!root.join("parent/file").exists());
        let bound = BoundPath::capture(&root, "file", Scope::Project, true).unwrap();
        let old = root.with_extension("old");
        std::fs::rename(&root, &old).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert!(write_bound(&bound, &None, "bad").is_err());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(old).unwrap();
    }
}
