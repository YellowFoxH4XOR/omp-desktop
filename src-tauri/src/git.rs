use crate::dto::{ChangedFile, ChangesSummary, GitFile};
use crate::error::{AppError, AppResult};
#[cfg(unix)]
use crate::fsat;
use crate::util;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

mod checked_write;
mod worktree;

use checked_write::*;
pub use worktree::*;

fn repo_root(cwd: &Path) -> AppResult<PathBuf> {
    let mut output = git_ok(cwd, &["rev-parse", "--show-toplevel"])?;
    if output.last() == Some(&b'\n') {
        output.pop();
    }
    if output.last() == Some(&b'\r') {
        output.pop();
    }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(output))
    };
    #[cfg(not(unix))]
    let path = PathBuf::from(
        String::from_utf8(output)
            .map_err(|_| AppError::new("Repository path is not valid UTF-8."))?,
    );
    std::fs::canonicalize(&path)
        .map_err(|e| AppError::new(format!("Could not resolve repository root: {e}")))
}

const MAX_FILE_BYTES: u64 = 1024 * 1024; // 1 MiB inline content cap

/// Repository-selection variables git inherits from the process environment.
/// `-C` does not override these, so an inherited value can redirect every git
/// invocation away from `cwd` (making clean files look changed or hiding real
/// changes). Every git invocation goes through `git_command`.
const GIT_SCOPE_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_PREFIX",
];

/// Base `git` command scoped to `cwd` and insulated from repository-selection
/// environment variables. Callers add subcommand args and stdio, then spawn.
fn git_command(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    for var in GIT_SCOPE_VARS {
        cmd.env_remove(var);
    }
    cmd.arg("--literal-pathspecs").arg("-C").arg(cwd);
    cmd
}

fn git(cwd: &Path, args: &[&str]) -> AppResult<std::process::Output> {
    let out = git_command(cwd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| AppError::new(format!("Could not run git: {e}")))?;
    Ok(out)
}

const MAX_CHANGED_FILE_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Collect bounded command output without allowing `Command::output` to buffer
/// an attacker-controlled status or diff response in memory.
fn git_ok_capped(cwd: &Path, args: &[&str], cap: usize) -> AppResult<Vec<u8>> {
    use std::io::Read;

    let mut child = git_command(cwd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| AppError::new(format!("Could not run git: {e}")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("Could not read git output."))?
        .take((cap + 1) as u64);
    let mut output = Vec::with_capacity(cap.min(64 * 1024));
    stdout
        .read_to_end(&mut output)
        .map_err(|e| AppError::new(format!("Could not read git output: {e}")))?;
    if output.len() > cap {
        let _ = child.kill();
        let _ = child.wait();
        return Err(AppError::new(format!(
            "git {} output exceeded the {cap}-byte safety limit.",
            args.first().copied().unwrap_or("")
        )));
    }
    let status = child
        .wait()
        .map_err(|e| AppError::new(format!("Could not finish git: {e}")))?;
    if !status.success() {
        return Err(AppError::new(format!(
            "git {} failed.",
            args.first().copied().unwrap_or("")
        )));
    }
    Ok(output)
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

/// Most files an `@` mention list returns; beyond this it says it's partial.
pub const MAX_LISTED_FILES: usize = 20_000;
const MAX_LISTING_BYTES: usize = 4 * 1024 * 1024;
const MAX_WALK_DEPTH: usize = 16;
/// Folders a plain directory walk never descends into.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".svelte-kit",
    ".turbo",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".gradle",
    "Pods",
    "DerivedData",
    "coverage",
];

/// Files under `cwd` (all subfolders), relative to it, for `@` mentions. In a
/// Git checkout this is tracked plus untracked-but-not-ignored files; outside
/// one it is a bounded walk that skips build and dependency folders. Returns
/// the list and whether it was cut off.
pub fn list_files(cwd: &Path) -> AppResult<(Vec<String>, bool)> {
    if is_repo(cwd) {
        if let Ok(listing) = list_git_files(cwd) {
            return Ok(listing);
        }
    }
    let mut files = Vec::new();
    let mut truncated = false;
    walk(cwd, Path::new(""), 0, &mut files, &mut truncated);
    files.sort();
    Ok((files, truncated))
}

fn list_git_files(cwd: &Path) -> AppResult<(Vec<String>, bool)> {
    use std::io::Read;
    let mut child = git_command(cwd)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--deduplicate",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| AppError::new(format!("Could not run git: {e}")))?;
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("Could not read git output."))?
        .take((MAX_LISTING_BYTES + 1) as u64)
        .read_to_end(&mut output)?;
    let mut truncated = output.len() > MAX_LISTING_BYTES;
    if truncated {
        let _ = child.kill();
    }
    let status = child.wait()?;
    if !truncated && !status.success() {
        return Err(AppError::new("git ls-files failed."));
    }
    output.truncate(MAX_LISTING_BYTES);
    let mut files: Vec<String> = output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok())
        .map(String::from)
        .collect();
    if truncated {
        files.pop(); // The last entry may be cut mid-name.
    }
    files.sort();
    if files.len() > MAX_LISTED_FILES {
        files.truncate(MAX_LISTED_FILES);
        truncated = true;
    }
    Ok((files, truncated))
}

fn walk(root: &Path, relative: &Path, depth: usize, files: &mut Vec<String>, truncated: &mut bool) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root.join(relative)) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if files.len() >= MAX_LISTED_FILES {
            *truncated = true;
            return;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = relative.join(name);
        if kind.is_dir() {
            if !SKIPPED_DIRS.contains(&name) {
                walk(root, &path, depth + 1, files, truncated);
            }
        } else if kind.is_file() {
            files.push(path.to_string_lossy().into_owned());
        }
    }
}

/// Whether HEAD points at a commit (a new repository has none yet).
pub fn has_commits(cwd: &Path) -> bool {
    git(cwd, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub fn is_repo(cwd: &Path) -> bool {
    git(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}
/// Absolute path of a project file the UI may hand to the OS opener.
/// Git projects may only open paths Git currently reports as changed.
pub fn openable_path(cwd: &Path, rel: &str) -> AppResult<PathBuf> {
    let base = if is_repo(cwd) {
        repo_root(cwd)?
    } else {
        std::fs::canonicalize(cwd)
            .map_err(|e| AppError::new(format!("Could not resolve project directory: {e}")))?
    };
    let abs = validate_repo_path_at(&base, rel)?;
    if is_repo(cwd) {
        ensure_changed_path_at(&base, rel)?;
    }
    if !abs.exists() {
        return Err(AppError::new("File does not exist."));
    }
    Ok(abs)
}
fn validate_repo_path_at(base: &Path, rel: &str) -> AppResult<PathBuf> {
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
    let base = std::fs::canonicalize(base)
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
    let Ok(root) = repo_root(cwd) else {
        return Ok(ChangesSummary {
            is_repo: false,
            branch: None,
            files: vec![],
            additions: 0,
            deletions: 0,
        });
    };
    status_at(&root)
}

fn status_at(root: &Path) -> AppResult<ChangesSummary> {
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|b| !b.is_empty() && b != "HEAD");
    let porcelain = git_ok_capped(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        MAX_CHANGED_FILE_OUTPUT_BYTES,
    )?;
    let has_head = git(root, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    let numstat_map = if has_head {
        parse_numstat(&git_ok_capped(
            root,
            &["diff", "--numstat", "-z", "HEAD", "--"],
            MAX_CHANGED_FILE_OUTPUT_BYTES,
        )?)
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
            if let Some((lines, binary)) = count_untracked_lines(&root.join(&f.path)) {
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
fn ensure_changed_path_at(root: &Path, rel: &str) -> AppResult<()> {
    let porcelain = git_ok_capped(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        MAX_CHANGED_FILE_OUTPUT_BYTES,
    )?;
    if !parse_porcelain(&porcelain)
        .iter()
        .any(|file| file.path == rel)
    {
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

/// Cap on bytes scanned when counting untracked lines. A multi-gigabyte
/// artifact stays listed without blocking the review command on a full read;
/// lines are counted only within the scanned prefix.
const MAX_UNTRACKED_SCAN_BYTES: u64 = 256 * 1024;

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
    let mut scanned = 0u64;
    let mut last = 0u8;
    let mut had_bytes = false;
    while scanned < MAX_UNTRACKED_SCAN_BYTES {
        let want = (MAX_UNTRACKED_SCAN_BYTES - scanned).min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..want]).ok()?;
        if n == 0 {
            break;
        }
        if inspected < 8192 && buf[..n.min(8192 - inspected)].contains(&0) {
            return Some((0, true));
        }
        inspected += n;
        scanned += n as u64;
        had_bytes = true;
        last = buf[n - 1];
        lines += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
    }
    Some((lines + u64::from(had_bytes && last != b'\n'), false))
}

/// Parse `git status --porcelain=v1 -z` output.
/// Rename/copy entries are `XY new\0old\0`; the first path is the worktree
/// entry. `-z` emits raw bytes without quoting, so paths are matched bytewise
/// and only valid UTF-8 paths are reported (non-UTF-8 entries are skipped so
/// two distinct files can never collapse to one `String`).
fn parse_porcelain(data: &[u8]) -> Vec<ChangedFile> {
    let mut out = Vec::new();
    let mut fields = data.split(|b| *b == 0);
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let x = field[0];
        let y = field[1];
        if x == b'?' && y == b'?' {
            let Ok(path) = std::str::from_utf8(&field[3..]) else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            out.push(ChangedFile {
                path: path.to_string(),
                status: "untracked".to_string(),
                additions: 0,
                deletions: 0,
                binary: false,
            });
            continue;
        }
        if x == b'!' && y == b'!' {
            continue;
        }
        let is_rename = x == b'R' || y == b'R';
        let is_copy = x == b'C' || y == b'C';
        if is_rename || is_copy {
            // `-z` rename/copy: `XY new\0old\0`. Always consume the extra
            // source field even when the destination is skipped below.
            let _old = fields.next();
            let Ok(path) = std::str::from_utf8(&field[3..]) else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            out.push(ChangedFile {
                path: path.to_string(),
                status: if is_rename {
                    "renamed".to_string()
                } else {
                    "copied".to_string()
                },
                additions: 0,
                deletions: 0,
                binary: false,
            });
            continue;
        }
        let status = match (x as char, y as char) {
            ('A', _) | (_, 'A') => "added",
            ('D', _) | (_, 'D') => "deleted",
            ('T', _) | (_, 'T') => "typechange",
            ('U', _) | (_, 'U') => "conflicted",
            _ => "modified",
        };
        let Ok(path) = std::str::from_utf8(&field[3..]) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        out.push(ChangedFile {
            path: path.to_string(),
            status: status.to_string(),
            additions: 0,
            deletions: 0,
            binary: false,
        });
    }
    out
}

/// Parse `git diff --numstat -z` → map path → (adds, dels, binary).
/// Paths are matched bytewise and only valid UTF-8 paths are reported, so two
/// distinct files can never collapse to one lossy `String` key.
fn parse_numstat(data: &[u8]) -> std::collections::HashMap<String, (u64, u64, bool)> {
    fn split_record(field: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
        let mut parts = field.splitn(3, |byte| *byte == b'\t');
        Some((parts.next()?, parts.next()?, parts.next()?))
    }
    let mut fields = data.split(|byte| *byte == 0);
    let mut map = std::collections::HashMap::new();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let Some((a, d, p)) = split_record(field) else {
            continue;
        };
        let binary = a == b"-" && d == b"-";
        let parse_count = |raw: &[u8]| {
            std::str::from_utf8(raw)
                .ok()
                .and_then(|text| text.parse::<u64>().ok())
                .unwrap_or(0)
        };
        let adds = parse_count(a);
        let dels = parse_count(d);
        if p.is_empty() {
            // Rename: next two NUL fields are old path then new path.
            let _old_path = fields.next();
            let new_path = fields.next().unwrap_or_default();
            if !new_path.is_empty() {
                if let Ok(new_path) = std::str::from_utf8(new_path) {
                    map.insert(new_path.to_string(), (adds, dels, binary));
                }
            }
        } else if let Ok(p) = std::str::from_utf8(p) {
            map.insert(p.to_string(), (adds, dels, binary));
        }
    }
    map
}

/// Content hash matching `git hash-object` semantics, computed via git itself
/// so hashing semantics never drift. The reader is streamed in fixed-size
/// chunks, keeping memory bounded even for multi-gigabyte worktree files.
fn hash_reader(cwd: &Path, reader: &mut impl std::io::Read) -> AppResult<String> {
    let mut child = git_command(cwd)
        .args(["hash-object", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(format!("Could not run git: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppError::new("Could not open git hash input."))?;
    let copied = std::io::copy(reader, &mut stdin);
    drop(stdin);
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::new(format!("Could not hash content: {e}")))?;
    let copied = copied.map_err(|e| AppError::new(format!("Could not hash content: {e}")))?;
    if !out.status.success() {
        return Err(AppError::new(format!(
            "Could not hash content: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    if copied > MAX_FILE_BYTES {
        // The hash remains useful to the hunk writer, but no file-sized
        // allocation was made to produce it.
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn hash_bytes(cwd: &Path, bytes: &[u8]) -> AppResult<String> {
    hash_reader(cwd, &mut &bytes[..])
}

fn hash_file(cwd: &Path, abs: &Path) -> AppResult<String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(abs) {
        Ok(mut file) => hash_reader(cwd, &mut file),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => hash_bytes(cwd, b""),
        Err(e) => Err(AppError::new(format!("Could not read file: {e}"))),
    }
}
/// Read the HEAD blob through the worktree filter path (`text`/`eol`
/// attributes, `core.autocrlf`) so the old side of the diff matches how the
/// file materializes in the worktree. Returns `None` when filters do not
/// apply, letting the caller keep the raw blob.
fn old_through_filters(root: &Path, rel: &str, object: &str) -> Option<Vec<u8>> {
    let out = git(
        root,
        &["cat-file", "--filters", &format!("--path={rel}"), object],
    )
    .ok()?;
    if !out.status.success() || !out.stderr.is_empty() {
        return None;
    }
    Some(out.stdout)
}

/// Read reviewable bytes and their content hash from one open descriptor.
/// On Unix the descriptor comes from the existing no-follow checked-file
/// helpers, so a symlink swap between the link check and the read cannot
/// serve bytes from outside the repository. On other platforms the same
/// `hash_file` bytes are used for both identity and content.
#[cfg(unix)]
fn read_reviewed_bytes(root: &Path, abs: &Path) -> AppResult<(Vec<u8>, String)> {
    use std::io::Read;
    let mut file = open_checked_file_no_create(root, abs)?;
    let metadata = file
        .metadata()
        .map_err(|e| AppError::new(format!("Could not inspect file: {e}")))?;
    if !metadata.is_file() {
        return Err(AppError::new("Only regular files can be reviewed."));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| AppError::new(format!("Could not read file: {e}")))?;
    let hash = hash_reader(root, &mut &bytes[..])?;
    Ok((bytes, hash))
}

#[cfg(not(unix))]
fn read_reviewed_bytes(root: &Path, abs: &Path) -> AppResult<(Vec<u8>, String)> {
    let bytes =
        std::fs::read(abs).map_err(|e| AppError::new(format!("Could not read file: {e}")))?;
    let hash = hash_reader(root, &mut &bytes[..])?;
    Ok((bytes, hash))
}

pub fn file(cwd: &Path, rel: &str) -> AppResult<GitFile> {
    let root = repo_root(cwd)?;
    let abs = validate_repo_path_at(&root, rel)?;
    ensure_changed_path_at(&root, rel)?;
    if linked_entry(&root, rel, &abs)? {
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
    let old_size = git(&root, &["cat-file", "-s", &object])
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|size| size.trim().parse::<u64>().ok());
    let mut old_too_large = old_size.is_some_and(|size| size > MAX_FILE_BYTES);
    let old = if old_size.is_some() && !old_too_large {
        // Prefer the worktree-filtered bytes; keep the raw blob only when
        // filters are irrelevant or fail.
        let filtered = old_through_filters(&root, rel, &object);
        let raw;
        let bytes = match filtered.as_ref() {
            Some(bytes) => bytes,
            None => {
                raw = git_ok(&root, &["show", &object])?;
                &raw
            }
        };
        if bytes.len() as u64 > MAX_FILE_BYTES {
            old_too_large = true;
            Vec::new()
        } else {
            bytes.clone()
        }
    } else {
        Vec::new()
    };
    let meta = match std::fs::symlink_metadata(&abs) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(AppError::new(format!("Could not inspect file: {e}"))),
    };
    // The no-follow reader fails closed on symlinks/directories, so a swap
    // between `linked_entry` and this read surfaces as an error rather than
    // outside-repo bytes.
    let (current_bytes, cur_too_large, current_hash) = match &meta {
        Some(m) if m.len() > MAX_FILE_BYTES => (Vec::new(), true, hash_file(&root, &abs)?),
        Some(_) => {
            let (bytes, hash) = read_reviewed_bytes(&root, &abs)?;
            (bytes, false, hash)
        }
        None => (Vec::new(), false, hash_bytes(&root, b"")?),
    };
    let too_large = cur_too_large || old_too_large;
    let binary = util::looks_binary(&current_bytes)
        || util::looks_binary(&old)
        || std::str::from_utf8(&current_bytes).is_err()
        || std::str::from_utf8(&old).is_err();
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

fn staged_rename_source(root: &Path, new_path: &str) -> AppResult<Option<String>> {
    let data = match git_ok_capped(
        root,
        &[
            "diff",
            "--cached",
            "--name-status",
            "-z",
            "-M",
            "HEAD",
            "--",
        ],
        MAX_CHANGED_FILE_OUTPUT_BYTES,
    ) {
        Ok(data) => data,
        _ => return Ok(None),
    };
    let mut fields = data
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(status) = fields.next() {
        let renamed = status.first() == Some(&b'R') || status.first() == Some(&b'C');
        let first = fields.next().unwrap_or_default();
        if !renamed {
            if first == new_path.as_bytes() {
                return Ok(None);
            }
            continue;
        }
        let new = fields.next().unwrap_or_default();
        if status.first() == Some(&b'R') && new == new_path.as_bytes() {
            let Ok(source) = std::str::from_utf8(first) else {
                return Ok(None);
            };
            return Ok(Some(source.to_owned()));
        }
    }
    Ok(None)
}

/// Revert one file to HEAD. Files absent from HEAD are moved to Trash, never
/// permanently deleted as an implicit fallback. Staged additions are unstaged.
/// When `expected_hash` is `Some`, the current worktree bytes are hashed
/// (the same hash as `GitFile.currentHash`) immediately before any
/// destructive step and a mismatch aborts with a refresh error.
pub fn revert_file(cwd: &Path, rel: &str, expected_hash: Option<&str>) -> AppResult<()> {
    revert_file_guarded(cwd, rel, expected_hash)
}

/// Snapshot the worktree bytes for the revert guard through `hash_file`, the
/// same hash used for `GitFile.currentHash` (absent paths hash as empty).
fn revert_snapshot(root: &Path, abs: &Path) -> AppResult<String> {
    hash_file(root, abs)
}

/// Verify the current worktree bytes still match the caller's hash immediately
/// before any destructive git/filesystem action.
fn check_revert_guard(root: &Path, abs: &Path, expected_hash: Option<&str>) -> AppResult<()> {
    if let Some(expected) = expected_hash {
        let current = match revert_snapshot(root, abs) {
            Ok(hash) => hash,
            // A symlink swap or unreadable file fails closed with the same
            // refresh message rather than proceeding destructively.
            Err(_) => {
                return Err(AppError::new(
                    "The file changed on disk; refresh the file list and try again.",
                ));
            }
        };
        if current != expected {
            return Err(AppError::new(
                "The file changed on disk; refresh the file list and try again.",
            ));
        }
    }
    Ok(())
}

/// Reject gitlink (submodule) entries. `git checkout HEAD -- <rel>` can leave
/// a changed submodule dirty while reporting success, so revert refuses them
/// instead of reporting Ok while the entry stays dirty.
fn reject_gitlink(root: &Path, rel: &str) -> AppResult<()> {
    for args in [
        ["ls-files", "--stage", "--", rel],
        ["ls-tree", "HEAD", "--", rel],
    ] {
        let output = git(root, &args)?;
        if output.status.success() && output.stdout.starts_with(b"160000 ") {
            return Err(AppError::new(
                "Submodule changes cannot be reverted file-by-file. Revert the submodule from its own checkout.",
            ));
        }
    }
    Ok(())
}

fn revert_file_guarded(cwd: &Path, rel: &str, expected_hash: Option<&str>) -> AppResult<()> {
    let root = repo_root(cwd)?;
    let abs = validate_repo_path_at(&root, rel)?;
    ensure_changed_path_at(&root, rel)?;
    reject_gitlink(&root, rel)?;
    // Early mismatch check before the read-only probes below; every
    // destructive branch re-checks immediately before mutating, so a change
    // between this probe and the destructive step still aborts.
    check_revert_guard(&root, &abs, expected_hash)?;
    let in_head = git(&root, &["cat-file", "-e", &format!("HEAD:{rel}")])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if in_head {
        check_revert_guard(&root, &abs, expected_hash)?;
        git_ok(&root, &["checkout", "HEAD", "--", rel])?;
        return Ok(());
    }

    // A staged rename has a source path in HEAD even though the destination
    // does not. Restore that source only when it will not overwrite new work.
    // The checkout below only creates the absent source path; the guard on
    // `rel` runs after these read-only probes, immediately before the first
    // destructive step touching `rel`.
    let rename_source = staged_rename_source(&root, rel)?;
    let mut restored_source: Option<(PathBuf, String)> = None;
    if let Some(source) = rename_source.as_deref() {
        let source_abs = validate_repo_path_at(&root, source)?;
        if std::fs::symlink_metadata(&source_abs).is_ok() {
            return Err(AppError::new(
                "The original path exists again. Move it aside before reverting this rename.",
            ));
        }
        git_ok(&root, &["checkout", "HEAD", "--", source])?;
        // Identity of the bytes this operation restored, so rollback below
        // never permanently deletes work another process wrote afterwards.
        let restored_hash = hash_file(&root, &source_abs)?;
        restored_source = Some((source_abs, restored_hash));
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
    let staged = git(&root, &["ls-files", "--error-unmatch", "--", rel])?
        .status
        .success();
    if staged {
        check_revert_guard(&root, &abs, expected_hash)?;
        git_ok(&root, &["rm", "-f", "--cached", "--", rel])?;
    }
    if exists {
        check_revert_guard(&root, &abs, expected_hash)?;
        if let Err(error) = trash::delete(&abs) {
            if let Some((source_abs, restored_hash)) = &restored_source {
                // Only remove the source this operation restored, and only
                // when it still holds those bytes; otherwise leave the
                // recreated work in place and report the Trash failure.
                let still_ours = hash_file(&root, source_abs)
                    .map(|hash| &hash == restored_hash)
                    .unwrap_or(false);
                if still_ours {
                    let _ = trash::delete(source_abs);
                }
                if let Some(source) = rename_source.as_deref() {
                    let _ = git(&root, &["rm", "-f", "--cached", "--", source]);
                }
            }
            let rollback = if staged {
                git(&root, &["add", "--", rel]).err()
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
    let root = repo_root(cwd)?;
    let abs = validate_repo_path_at(&root, rel)?;
    ensure_changed_path_at(&root, rel)?;
    if linked_entry(&root, rel, &abs)? {
        return Err(AppError::new(
            "Symbolic links cannot be reverted by hunk. Revert the whole file instead.",
        ));
    }
    write_checked_unix(&root, &abs, expected_hash, content)
}

#[cfg(test)]
mod tests;
