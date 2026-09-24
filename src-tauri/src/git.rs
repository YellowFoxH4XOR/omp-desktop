use crate::dto::{ChangedFile, ChangesSummary, GitFile};
use crate::error::{AppError, AppResult};
use crate::util;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const MAX_FILE_BYTES: u64 = 1024 * 1024; // 1 MiB inline content cap

fn git(cwd: &Path, args: &[&str]) -> AppResult<std::process::Output> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| AppError::new(format!("Could not run git: {e}")))?;
    Ok(out)
}

fn git_ok(cwd: &Path, args: &[&str]) -> AppResult<Vec<u8>> {
    let out = git(cwd, args)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::new(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            stderr.trim()
        )));
    }
    Ok(out.stdout)
}

pub fn is_repo(cwd: &Path) -> bool {
    git(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Validate a repo-relative entry path. A symlink at the final component is
/// allowed as an entry (Git/Trash can replace the link itself), but no parent
/// may escape the repository through a symlink.
pub fn validate_repo_path(cwd: &Path, rel: &str) -> AppResult<PathBuf> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(AppError::new("Path must be relative to the repository."));
    }
    for comp in rel_path.components() {
        match comp {
            Component::ParentDir => {
                return Err(AppError::new("Path cannot contain `..`."));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::new("Path must be relative to the repository."));
            }
            _ => {}
        }
    }
    let base = std::fs::canonicalize(cwd)
        .map_err(|e| AppError::new(format!("Could not resolve repository directory: {e}")))?;
    let candidate = base.join(rel_path);
    let mut probe = candidate
        .parent()
        .ok_or_else(|| AppError::new("Path is outside the repository."))?
        .to_path_buf();
    loop {
        if std::fs::symlink_metadata(&probe).is_ok() {
            break;
        }
        if !probe.pop() {
            return Err(AppError::new("Path is outside the repository."));
        }
    }
    // Canonicalization errors include dangling parent symlinks. Fail closed
    // instead of accepting a lexical path that a later write could follow.
    let resolved_parent = std::fs::canonicalize(&probe)
        .map_err(|_| AppError::new("Could not safely resolve the file's parent directory."))?;
    if !resolved_parent.starts_with(&base) {
        return Err(AppError::new(
            "Path escapes the repository through a symlink.",
        ));
    }
    Ok(candidate)
}

/// `git status --porcelain=v1 -z` + `git diff --numstat` merged into one summary.
pub fn status(cwd: &Path) -> AppResult<ChangesSummary> {
    if !is_repo(cwd) {
        return Ok(ChangesSummary {
            is_repo: false,
            branch: None,
            files: vec![],
            additions: 0,
            deletions: 0,
        });
    }
    let branch = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|b| !b.is_empty() && b != "HEAD");
    let porcelain = git_ok(
        cwd,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let has_head = git(cwd, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    let numstat_map = if has_head {
        parse_numstat(&git_ok(cwd, &["diff", "--numstat", "-z", "HEAD", "--"])?)
    } else {
        std::collections::HashMap::new()
    };
    let mut files = parse_porcelain(&porcelain);
    let mut additions = 0u64;
    let mut deletions = 0u64;
    for f in files.iter_mut() {
        if f.status == "untracked" || !has_head {
            // Stream large untracked files without loading them in memory.
            // Never follow an untracked symlink out of the repository.
            if let Some((lines, binary)) = count_untracked_lines(&cwd.join(&f.path)) {
                f.additions = lines;
                f.binary = binary;
            }
        } else if let Some((a, d, binary)) = numstat_map.get(&f.path) {
            f.additions = *a;
            f.deletions = *d;
            f.binary = *binary;
        }
        additions += f.additions;
        deletions += f.deletions;
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ChangesSummary {
        is_repo: true,
        branch,
        files,
        additions,
        deletions,
    })
}
/// Review commands operate only on files Git currently reports as changed.
/// This prevents the webview from turning a diff action into arbitrary writes.
fn ensure_changed_path(cwd: &Path, rel: &str) -> AppResult<()> {
    if !status(cwd)?.files.iter().any(|file| file.path == rel) {
        return Err(AppError::new(
            "This path is not in the current Git changes. Refresh the file list.",
        ));
    }
    Ok(())
}
/// A linked path is an entry, not editable text. Checking both working tree
/// and Git metadata covers deleted or staged links as well as live symlinks.
fn linked_entry(cwd: &Path, rel: &str, abs: &Path) -> AppResult<bool> {
    match std::fs::symlink_metadata(abs) {
        Ok(meta) if meta.file_type().is_symlink() => return Ok(true),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(AppError::new(format!("Could not inspect file: {e}"))),
    }
    for args in [
        ["ls-files", "--stage", "--", rel],
        ["ls-tree", "HEAD", "--", rel],
    ] {
        let output = git(cwd, &args)?;
        if output.status.success() && output.stdout.starts_with(b"120000 ") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Count working-tree lines in bounded memory. Nonregular paths, including
/// symlinks, are represented as binary rather than read through.
fn count_untracked_lines(path: &Path) -> Option<(u64, bool)> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return Some((0, true));
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 64 * 1024];
    let mut lines = 0u64;
    let mut inspected = 0usize;
    let mut last = 0u8;
    let mut had_bytes = false;
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        if inspected < 8192 && buf[..n.min(8192 - inspected)].contains(&0) {
            return Some((0, true));
        }
        inspected += n;
        had_bytes = true;
        last = buf[n - 1];
        lines += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
    }
    Some((lines + u64::from(had_bytes && last != b'\n'), false))
}

/// Parse `git status --porcelain=v1 -z` output.
/// Rename/copy entries store the new path first, then the original path.
fn parse_porcelain(data: &[u8]) -> Vec<ChangedFile> {
    let text = String::from_utf8_lossy(data);
    let mut fields = text.split('\0');
    let mut out = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let x = field.as_bytes()[0] as char;
        let y = field.as_bytes()[1] as char;
        let mut path = field[3..].to_string();
        let status = if x == '?' && y == '?' {
            "untracked"
        } else if x == '!' && y == '!' {
            fields.next();
            continue;
        } else {
            match (x, y) {
                ('R', _) | ('C', _) => {
                    // -z format: "XY new\0old\0"
                    let _old = fields.next();
                    if x == 'R' {
                        "renamed"
                    } else {
                        "copied"
                    }
                }
                ('A', _) | (_, 'A') => "added",
                ('D', _) | (_, 'D') => "deleted",
                ('T', _) | (_, 'T') => "typechange",
                ('U', _) | (_, 'U') => "conflicted",
                _ => "modified",
            }
        };
        if status == "deleted" {
            // keep path as-is
        }
        if path.is_empty() {
            continue;
        }
        // Strip quoting git may add for unusual names (porcelain -z does not quote).
        if path.starts_with('"') && path.ends_with('"') && path.len() > 1 {
            path = path[1..path.len() - 1].to_string();
        }
        out.push(ChangedFile {
            path,
            status: status.to_string(),
            additions: 0,
            deletions: 0,
            binary: false,
        });
    }
    out
}

/// Parse `git diff --numstat -z` → map path → (adds, dels, binary).
fn parse_numstat(data: &[u8]) -> std::collections::HashMap<String, (u64, u64, bool)> {
    let text = String::from_utf8_lossy(data);
    let mut fields = text.split('\0').peekable();
    let mut map = std::collections::HashMap::new();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut parts = field.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let binary = a == "-" && d == "-";
        let adds = a.parse::<u64>().unwrap_or(0);
        let dels = d.parse::<u64>().unwrap_or(0);
        if p.is_empty() {
            // Rename: next two NUL fields are old path then new path.
            let _old_path = fields.next();
            let new_path = fields.next().unwrap_or("").to_string();
            if !new_path.is_empty() {
                map.insert(new_path, (adds, dels, binary));
            }
        } else {
            map.insert(p.to_string(), (adds, dels, binary));
        }
    }
    map
}

/// Content hash matching `git hash-object` semantics, computed via git itself
/// so hashing semantics never drift.
fn hash_bytes(cwd: &Path, bytes: &[u8]) -> AppResult<String> {
    use std::io::Write;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["hash-object", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| AppError::new(format!("Could not run git: {e}")))?;
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(bytes)
        .map_err(|e| AppError::new(format!("Could not hash content: {e}")))?;
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::new(format!("Could not hash content: {e}")))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn hash_file(cwd: &Path, abs: &Path) -> AppResult<String> {
    match std::fs::read(abs) {
        Ok(bytes) => hash_bytes(cwd, &bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => hash_bytes(cwd, b""),
        Err(e) => Err(AppError::new(format!("Could not read file: {e}"))),
    }
}

/// HEAD version + current version + current hash for one file.
pub fn file(cwd: &Path, rel: &str) -> AppResult<GitFile> {
    let abs = validate_repo_path(cwd, rel)?;
    ensure_changed_path(cwd, rel)?;
    if linked_entry(cwd, rel, &abs)? {
        return Ok(GitFile {
            path: rel.to_string(),
            old: String::new(),
            current: String::new(),
            current_hash: String::new(),
            binary: true,
            too_large: false,
        });
    }
    let object = format!("HEAD:{rel}");
    let old_size = git(cwd, &["cat-file", "-s", &object])
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|size| size.trim().parse::<u64>().ok());
    let mut old_too_large = old_size.is_some_and(|size| size > MAX_FILE_BYTES);
    let old = if old_size.is_some() && !old_too_large {
        let bytes = git_ok(cwd, &["show", &object])?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            old_too_large = true;
            Vec::new()
        } else {
            bytes
        }
    } else {
        Vec::new()
    };
    let meta = match std::fs::metadata(&abs) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(AppError::new(format!("Could not inspect file: {e}"))),
    };
    let (current_bytes, cur_too_large) = match &meta {
        Some(m) if m.is_file() => {
            if m.len() > MAX_FILE_BYTES {
                (Vec::new(), true)
            } else {
                (
                    std::fs::read(&abs)
                        .map_err(|e| AppError::new(format!("Could not read file: {e}")))?,
                    false,
                )
            }
        }
        _ => (Vec::new(), false),
    };
    let too_large = cur_too_large || old_too_large;
    let binary = util::looks_binary(&current_bytes)
        || util::looks_binary(&old)
        || std::str::from_utf8(&current_bytes).is_err()
        || std::str::from_utf8(&old).is_err();
    let current_hash = hash_file(cwd, &abs)?;
    let (old, current) = if binary || too_large {
        (String::new(), String::new())
    } else {
        (
            String::from_utf8(old).expect("validated"),
            String::from_utf8(current_bytes).expect("validated"),
        )
    };
    Ok(GitFile {
        path: rel.to_string(),
        old,
        current,
        current_hash,
        binary,
        too_large,
    })
}

/// Revert one file to HEAD. Files absent from HEAD are moved to Trash, never
/// permanently deleted as an implicit fallback. Staged additions are unstaged.
pub fn revert_file(cwd: &Path, rel: &str) -> AppResult<()> {
    let abs = validate_repo_path(cwd, rel)?;
    ensure_changed_path(cwd, rel)?;
    // Does HEAD contain this path?
    let in_head = git(cwd, &["cat-file", "-e", &format!("HEAD:{rel}")])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if in_head {
        git_ok(cwd, &["checkout", "HEAD", "--", rel])?;
        return Ok(());
    }
    // A new file may be staged, untracked, or a dangling symlink. Reject a
    // Trash failure rather than silently deleting data permanently.
    let exists = match std::fs::symlink_metadata(&abs) {
        Ok(meta) if meta.is_dir() => {
            return Err(AppError::new(
                "Refusing to remove a directory; revert files only.",
            ));
        }
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(AppError::new(format!("Could not inspect file: {e}"))),
    };
    let staged = git(cwd, &["ls-files", "--error-unmatch", "--", rel])?
        .status
        .success();
    if staged {
        git_ok(cwd, &["rm", "-f", "--cached", "--", rel])?;
    }
    if exists {
        if let Err(error) = trash::delete(&abs) {
            let rollback = if staged {
                git_ok(cwd, &["add", "--", rel]).err()
            } else {
                None
            };
            let detail = rollback
                .map(|e| format!(" The Git index could not be restored: {e}"))
                .unwrap_or_default();
            return Err(AppError::new(format!(
                "Could not move the new file to Trash: {error}. No file was deleted.{detail}"
            )));
        }
    }
    Ok(())
}

/// Write `content` to `rel` only when the file's current hash still equals
/// `expected_hash` (guards against clobbering unseen edits).
pub fn write_if_unchanged(
    cwd: &Path,
    rel: &str,
    expected_hash: &str,
    content: &str,
) -> AppResult<()> {
    let abs = validate_repo_path(cwd, rel)?;
    ensure_changed_path(cwd, rel)?;
    if linked_entry(cwd, rel, &abs)? {
        return Err(AppError::new(
            "Symbolic links cannot be reverted by hunk. Revert the whole file instead.",
        ));
    }
    let current = hash_file(cwd, &abs)?;
    if current != expected_hash {
        return Err(AppError::new(
            "The file changed on disk since you loaded it. Refresh the diff and try again.",
        ));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::new(format!("Could not create directories: {e}")))?;
    }
    std::fs::write(&abs, content)
        .map_err(|e| AppError::new(format!("Could not write file: {e}")))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `git status --porcelain=v1 -z` emits "XY new\0old\0" for renames.
    #[test]
    fn porcelain_rename_order() {
        let data = b"R  newname.txt\0oldname.txt\0M  src/a.rs\0?? scratch.md\0";
        let files = parse_porcelain(data);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "newname.txt");
        assert_eq!(files[0].status, "renamed");
        assert_eq!(files[1].path, "src/a.rs");
        assert_eq!(files[1].status, "modified");
        assert_eq!(files[2].path, "scratch.md");
        assert_eq!(files[2].status, "untracked");
    }

    /// `git diff --numstat -z` emits an empty path field then OLD then NEW.
    #[test]
    fn numstat_rename_order() {
        let data = b"0\t0\t\0oldname.txt\0newname.txt\03\t2\tsrc/a.rs\0-\t-\timg.png\0";
        let map = parse_numstat(data);
        assert_eq!(map.get("newname.txt"), Some(&(0, 0, false)));
        assert_eq!(map.get("src/a.rs"), Some(&(3, 2, false)));
        assert_eq!(map.get("img.png"), Some(&(0, 0, true)));
        assert!(!map.contains_key("oldname.txt"));
    }

    /// Path validation rejects escapes and accepts normal relative paths.
    #[test]
    fn repo_path_validation() {
        let dir = std::env::temp_dir().join(format!("omp-git-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(validate_repo_path(&dir, "src/a.rs").is_ok());
        assert!(validate_repo_path(&dir, "../escape.rs").is_err());
        assert!(validate_repo_path(&dir, "/abs/path.rs").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[cfg(unix)]
    #[test]
    fn final_symlink_entry_is_safe_but_intermediate_escape_is_not() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("omp-git-link-guard-{}", uuid::Uuid::new_v4()));
        let outside = dir.with_extension("outside");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&outside, "private\n").unwrap();
        symlink(&outside, dir.join("link.txt")).unwrap();
        symlink(outside.parent().unwrap(), dir.join("escape")).unwrap();
        assert!(
            validate_repo_path(&dir, "link.txt").is_ok(),
            "reverting the link entry never opens its target"
        );
        assert!(validate_repo_path(
            &dir,
            &format!("escape/{}", outside.file_name().unwrap().to_string_lossy())
        )
        .is_err());
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_file(outside).unwrap();
    }

    /// An unborn repository has no HEAD but still has reviewable staged and
    /// untracked files. The review panel must count their current lines.
    #[test]
    fn unborn_head_lists_staged_and_untracked() {
        let dir = std::env::temp_dir().join(format!("omp-git-unborn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&dir)
            .arg("init")
            .arg("-q")
            .status()
            .unwrap()
            .success());
        std::fs::write(dir.join("staged.txt"), "alpha\nbeta\n").unwrap();
        std::fs::write(dir.join("loose.txt"), "gamma\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["add", "staged.txt"])
            .status()
            .unwrap()
            .success());
        let changes = status(&dir).unwrap();
        assert_eq!(changes.files.len(), 2);
        assert_eq!(changes.additions, 3);
        assert_eq!(changes.deletions, 0);
        assert_eq!(file(&dir, "staged.txt").unwrap().old, "");
        assert!(file(&dir, ".git/config").is_err());
        assert!(write_if_unchanged(&dir, ".git/config", "", "override").is_err());
        #[cfg(unix)]
        {
            let outside = dir.with_extension("outside");
            std::fs::write(&outside, "private\ncontent\n").unwrap();
            std::os::unix::fs::symlink(&outside, dir.join("outside-link")).unwrap();
            let changes = status(&dir).unwrap();
            let link = changes
                .files
                .iter()
                .find(|file| file.path == "outside-link")
                .unwrap();
            assert!(link.binary);
            assert_eq!(link.additions, 0);
            std::fs::remove_file(outside).unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn manual_edit_and_reverts_follow_current_git_state() {
        let dir = std::env::temp_dir().join(format!("omp-git-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("example.txt");
        assert!(Command::new("git")
            .arg("-C")
            .arg(&dir)
            .arg("init")
            .arg("-q")
            .status()
            .unwrap()
            .success());
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["add", "."])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args([
                "-c",
                "user.name=QA",
                "-c",
                "user.email=qa@localhost",
                "commit",
                "-qm",
                "Base"
            ])
            .status()
            .unwrap()
            .success());

        std::fs::write(&path, "alpha\ngamma\n").unwrap();
        let changes = status(&dir).unwrap();
        assert_eq!((changes.additions, changes.deletions), (1, 1));
        let diff = file(&dir, "example.txt").unwrap();
        assert_eq!(diff.old, "alpha\nbeta\n");
        assert_eq!(diff.current, "alpha\ngamma\n");
        assert!(write_if_unchanged(&dir, "example.txt", "stale-hash", &diff.old).is_err());
        write_if_unchanged(&dir, "example.txt", &diff.current_hash, &diff.old).unwrap();
        assert!(status(&dir).unwrap().files.is_empty());

        std::fs::write(&path, "alpha\ndelta\n").unwrap();
        revert_file(&dir, "example.txt").unwrap();
        assert!(status(&dir).unwrap().files.is_empty());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha\nbeta\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn isolated_worktree_does_not_mix_active_checkout_edits() {
        let root = std::env::temp_dir().join(format!("omp-git-isolation-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let isolated = root.join("isolated");
        std::fs::create_dir_all(&source).unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        std::fs::write(source.join("shared.txt"), "base\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["add", "."])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args([
                "-c",
                "user.name=QA",
                "-c",
                "user.email=qa@localhost",
                "commit",
                "-qm",
                "Base"
            ])
            .status()
            .unwrap()
            .success());

        std::fs::write(source.join("shared.txt"), "thread A in progress\n").unwrap();
        create_worktree(&source, &isolated).unwrap();
        assert_eq!(
            std::fs::read_to_string(isolated.join("shared.txt")).unwrap(),
            "base\n"
        );
        std::fs::write(isolated.join("shared.txt"), "thread B in progress\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("shared.txt")).unwrap(),
            "thread A in progress\n"
        );
        assert_eq!(status(&isolated).unwrap().files.len(), 1);
        assert_eq!(status(&source).unwrap().files.len(), 1);

        assert!(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["worktree", "remove", "--force"])
            .arg(&isolated)
            .status()
            .unwrap()
            .success());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn changed_symlink_cannot_revert_a_hunk_through_its_target() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("omp-git-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        std::fs::write(dir.join("first.txt"), "original\n").unwrap();
        std::fs::write(dir.join("second.txt"), "preserve this\n").unwrap();
        symlink("first.txt", dir.join("link.txt")).unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["add", "."])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args([
                "-c",
                "user.name=QA",
                "-c",
                "user.email=qa@localhost",
                "commit",
                "-qm",
                "Base"
            ])
            .status()
            .unwrap()
            .success());
        std::fs::remove_file(dir.join("link.txt")).unwrap();
        symlink("second.txt", dir.join("link.txt")).unwrap();
        let diff = file(&dir, "link.txt").unwrap();
        assert!(
            diff.binary,
            "linked files must not expose a text hunk editor"
        );
        assert!(write_if_unchanged(&dir, "link.txt", &diff.current_hash, "overwrite").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("second.txt")).unwrap(),
            "preserve this\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
