//! Private worktrees for isolated threads.

use super::*;

/// Intern consumes an approved repository capability, not a mutable `-C`
/// pathname. Project hooks are disabled for this app-controlled operation.
pub fn create_worktree_bound(
    repo: &crate::intern_files::BoundDirectory,
    dest: &Path,
) -> AppResult<()> {
    util::ensure_private_directory(&util::pidesk_root(), &util::pidesk_root().join("worktrees"))?;
    let mut command = git_command(Path::new("."));
    let _directory = repo.configure_command(&mut command)?;
    let output = command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "worktree",
            "add",
            "--detach",
        ])
        .arg(dest)
        .arg("HEAD")
        .stdin(std::process::Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(AppError::new(
            "Could not create the approved isolated worktree.",
        ));
    }
    repo.verify()?;
    Ok(())
}

/// Create an app-owned detached worktree for isolated threads.
pub fn create_worktree(repo: &Path, dest: &Path) -> AppResult<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::new(format!("Could not create worktree directory: {e}")))?;
    }
    let out = git(
        repo,
        &[
            "worktree",
            "add",
            "--detach",
            &dest.to_string_lossy(),
            "HEAD",
        ],
    )?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::new(format!(
            "Could not create an isolated worktree: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

/// Remove only a direct child of πDesk's private worktrees directory. The
/// stored path is metadata, never permission to delete an arbitrary checkout.
pub fn private_worktree_path(path: &Path) -> AppResult<PathBuf> {
    util::check_owned_path(&util::pidesk_root(), true)?;
    private_worktree_path_at(path, &util::pidesk_root().join("worktrees"))
}

pub(super) fn private_worktree_path_at(path: &Path, base: &Path) -> AppResult<PathBuf> {
    util::check_owned_path(base, true)?;
    let base = std::fs::canonicalize(base)?;
    let path = util::normalize_path(path);
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("This worktree does not belong to πDesk."))?;
    if std::fs::canonicalize(parent)? != base {
        return Err(AppError::new("This worktree does not belong to πDesk."));
    }
    let path = base.join(
        path.file_name()
            .ok_or_else(|| AppError::new("This worktree does not belong to πDesk."))?,
    );
    if path.exists() {
        util::check_owned_path(&path, true)?;
        if std::fs::canonicalize(&path)? != path {
            return Err(AppError::new("This worktree does not belong to πDesk."));
        }
    } else if std::fs::symlink_metadata(&path).is_ok() {
        return Err(AppError::new("This worktree does not belong to πDesk."));
    }
    Ok(path)
}

/// Include ignored and non-UTF-8 entries: force-removing a worktree can
/// discard those too, even though the normal Changes panel omits them.
pub fn worktree_changed_entries(path: &Path) -> AppResult<usize> {
    let porcelain = git_ok_capped(
        path,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
        ],
        MAX_CHANGED_FILE_OUTPUT_BYTES,
    )?;
    let mut count = 0;
    let mut fields = porcelain.split(|byte| *byte == 0);
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        count += 1;
        if field[0] == b'R' || field[1] == b'R' || field[0] == b'C' || field[1] == b'C' {
            fields.next(); // rename source, not an extra changed entry
        }
    }
    Ok(count)
}

pub fn remove_worktree(repo: &Path, path: &Path, force: bool) -> AppResult<()> {
    let path = private_worktree_path(path)?;
    remove_worktree_checked(repo, &path, force)
}

pub(super) fn remove_worktree_checked(repo: &Path, path: &Path, force: bool) -> AppResult<()> {
    if path.exists() {
        if !force && worktree_changed_entries(path)? > 0 {
            return Err(AppError::new("The isolated worktree has changes. Confirm discarding them before deleting this thread."));
        }
        let mut command = git_command(repo);
        command.args(["worktree", "remove"]);
        if force {
            command.arg("--force");
        }
        let output = command
            .arg(path)
            .stdin(std::process::Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(AppError::new(if force {
                "Could not remove the isolated worktree. Check for locked or nested worktrees and try again."
            } else {
                "The isolated worktree has changes. Confirm discarding them before deleting this thread."
            }));
        }
    }
    git_ok(repo, &["worktree", "prune"])?;
    Ok(())
}
