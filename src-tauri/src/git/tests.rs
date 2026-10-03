use super::*;

#[test]
fn a_new_repository_has_no_commits_until_the_first_one() {
    let root = std::env::temp_dir().join(format!("pidesk-commits-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(!has_commits(&root));
    git(&root, &["init", "-q"]).unwrap();
    assert!(!has_commits(&root));
    std::fs::write(root.join("a.txt"), "a").unwrap();
    git(&root, &["add", "a.txt"]).unwrap();
    git(
        &root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "first",
        ],
    )
    .unwrap();
    assert!(has_commits(&root));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_listing_covers_subfolders_and_skips_dependencies() {
    let root = std::env::temp_dir().join(format!("pidesk-files-{}", uuid::Uuid::new_v4()));
    for path in ["src/lib", "node_modules/pkg", "docs/deep/deeper"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    for file in [
        "README.md",
        "src/main.ts",
        "src/lib/util.ts",
        "node_modules/pkg/index.js",
        "docs/deep/deeper/notes.md",
    ] {
        std::fs::write(root.join(file), "x").unwrap();
    }
    let (files, truncated) = list_files(&root).unwrap();
    assert!(!truncated);
    assert_eq!(
        files,
        vec![
            "README.md",
            "docs/deep/deeper/notes.md",
            "src/lib/util.ts",
            "src/main.ts"
        ]
    );
    // In a Git checkout, ignored files are left out and new files included.
    git(&root, &["init", "-q"]).unwrap();
    std::fs::write(root.join(".gitignore"), "docs/\nnode_modules/\n").unwrap();
    let (files, _) = list_files(&root).unwrap();
    assert_eq!(
        files,
        vec![".gitignore", "README.md", "src/lib/util.ts", "src/main.ts"]
    );
    // A subfolder lists only its own tree, relative to itself.
    let (files, _) = list_files(&root.join("src")).unwrap();
    assert_eq!(files, vec!["lib/util.ts", "main.ts"]);
    std::fs::remove_dir_all(root).unwrap();
}

use std::io::Write;

#[cfg(unix)]
#[test]
fn absent_file_creation_preserves_concurrent_destination_and_cleans_temp() {
    let root =
        std::env::temp_dir().join(format!("pidesk-git-create-race-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let parent = std::fs::File::open(&root).unwrap();
    let temp = c_path(Path::new("pending.tmp")).unwrap();
    let name = c_path(Path::new("new.txt")).unwrap();
    std::fs::write(root.join("pending.tmp"), "ours\n").unwrap();
    std::fs::write(root.join("new.txt"), "theirs\n").unwrap();

    let result = atomic_replace(
        &parent,
        &temp,
        &name,
        &root,
        &root.join("new.txt"),
        "",
        false,
    );
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("new.txt")).unwrap(),
        "theirs\n"
    );
    assert!(!root.join("pending.tmp").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_file_git_output_is_hard_capped() {
    let root = std::env::temp_dir().join(format!("pidesk-git-output-cap-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "init", "-q"])
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join("changed.txt"), "changed\n").unwrap();
    let error = git_ok_capped(
        &root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        1,
    )
    .unwrap_err();
    assert!(error.to_string().contains("safety limit"));
    std::fs::remove_dir_all(root).unwrap();
}
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
    let data = b"0\t0\t\0oldname.txt\0newname.txt\x003\t2\tsrc/a.rs\0-\t-\timg.png\0";
    let map = parse_numstat(data);
    assert_eq!(map.get("newname.txt"), Some(&(0, 0, false)));
    assert_eq!(map.get("src/a.rs"), Some(&(3, 2, false)));
    assert_eq!(map.get("img.png"), Some(&(0, 0, true)));
    assert!(!map.contains_key("oldname.txt"));
}

/// `-z` porcelain never quotes: a file literally named `"secret"` must
/// not collapse onto `secret`.
#[test]
fn porcelain_keeps_literal_quotes() {
    let data = b" M \"secret\"\0";
    let files = parse_porcelain(data);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "\"secret\"");
    assert_eq!(files[0].status, "modified");
}

/// Rename/copy detection looks at both columns: a worktree rename (`RM`)
/// still consumes the extra source field.
#[test]
fn porcelain_worktree_rename_consumes_source() {
    let data = b"RM new.txt\0old.txt\0 M keep.txt\0";
    let files = parse_porcelain(data);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "new.txt");
    assert_eq!(files[0].status, "renamed");
    assert_eq!(files[1].path, "keep.txt");
    assert_eq!(files[1].status, "modified");
}

/// Non-UTF-8 paths are skipped so two distinct files can never share one
/// lossy `String` key.
#[test]
fn porcelain_skips_non_utf8_paths() {
    let mut data = b" M ok.txt\0".to_vec();
    data.extend_from_slice(b" M \xff.txt\0");
    let files = parse_porcelain(&data);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "ok.txt");
}

/// Path validation rejects escapes and accepts normal relative paths.
#[test]
fn repo_path_validation() {
    let dir = std::env::temp_dir().join(format!("pidesk-git-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(validate_repo_path_at(&dir, "src/a.rs").is_ok());
    assert!(validate_repo_path_at(&dir, "../escape.rs").is_err());
    assert!(validate_repo_path_at(&dir, "/abs/path.rs").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
#[cfg(unix)]
#[test]
fn final_symlink_entry_is_safe_but_intermediate_escape_is_not() {
    use std::os::unix::fs::symlink;
    let dir = std::env::temp_dir().join(format!("pidesk-git-link-guard-{}", uuid::Uuid::new_v4()));
    let outside = dir.with_extension("outside");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&outside, "private\n").unwrap();
    symlink(&outside, dir.join("link.txt")).unwrap();
    symlink(outside.parent().unwrap(), dir.join("escape")).unwrap();
    assert!(
        validate_repo_path_at(&dir, "link.txt").is_ok(),
        "reverting the link entry never opens its target"
    );
    assert!(validate_repo_path_at(
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
    let dir = std::env::temp_dir().join(format!("pidesk-git-unborn-{}", uuid::Uuid::new_v4()));
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

/// The replacement is created under the umask; an edit must not change
/// the permissions the file already had.
#[cfg(unix)]
#[test]
fn hunk_edit_keeps_the_files_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("pidesk-git-mode-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.sh");
    git(&dir, &["init", "-q"]).unwrap();
    std::fs::write(&path, "one\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(
        &dir,
        &[
            "-c",
            "user.name=QA",
            "-c",
            "user.email=qa@localhost",
            "commit",
            "-qm",
            "Base",
        ],
    )
    .unwrap();
    std::fs::write(&path, "two\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o775)).unwrap();

    let diff = file(&dir, "run.sh").unwrap();
    write_if_unchanged(&dir, "run.sh", &diff.current_hash, "three\n").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "three\n");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o775);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn manual_edit_and_reverts_follow_current_git_state() {
    let dir = std::env::temp_dir().join(format!("pidesk-git-review-{}", uuid::Uuid::new_v4()));
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
    revert_file(&dir, "example.txt", None).unwrap();
    assert!(status(&dir).unwrap().files.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\nbeta\n");

    // The hash-guarded revert refuses to discard an edit made after the
    // diff was read, and succeeds while the caller's hash still matches.
    std::fs::write(&path, "alpha\nepsilon\n").unwrap();
    let stale = file(&dir, "example.txt").unwrap();
    std::fs::write(&path, "alpha\nzeta\n").unwrap();
    let error = revert_file(&dir, "example.txt", Some(&stale.current_hash)).unwrap_err();
    assert!(
        error.to_string().contains("changed on disk"),
        "unexpected error: {error}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\nzeta\n");
    let fresh = file(&dir, "example.txt").unwrap();
    revert_file(&dir, "example.txt", Some(&fresh.current_hash)).unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha\nbeta\n");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn private_worktree_removal_refuses_dirty_and_outside_paths() {
    let root =
        std::env::temp_dir().join(format!("pidesk-remove-worktree-{}", uuid::Uuid::new_v4()));
    let source = root.join("source");
    let base = root.join("worktrees");
    let dest = base.join("isolated");
    std::fs::create_dir_all(&source).unwrap();
    assert!(git(&source, &["init", "-q"]).unwrap().status.success());
    std::fs::write(source.join("file.txt"), "base\n").unwrap();
    git_ok(&source, &["add", "."]).unwrap();
    assert!(git(
        &source,
        &[
            "-c",
            "user.name=QA",
            "-c",
            "user.email=qa@localhost",
            "commit",
            "-qm",
            "Base"
        ]
    )
    .unwrap()
    .status
    .success());
    create_worktree(&source, &dest).unwrap();
    assert!(private_worktree_path_at(&source, &base).is_err());
    std::fs::write(dest.join("file.txt"), "modified\n").unwrap();
    let checked = private_worktree_path_at(&dest, &base).unwrap();
    assert_eq!(worktree_changed_entries(&dest).unwrap(), 1);
    assert!(remove_worktree_checked(&source, &checked, false).is_err());
    assert!(dest.exists());
    remove_worktree_checked(&source, &checked, true).unwrap();
    assert!(!dest.exists());
    std::fs::write(source.join(".git/info/exclude"), "ignored.txt\n").unwrap();
    create_worktree(&source, &dest).unwrap();
    std::fs::write(dest.join("ignored.txt"), "private data\n").unwrap();
    assert!(status(&dest).unwrap().files.is_empty());
    assert_eq!(worktree_changed_entries(&dest).unwrap(), 1);
    assert!(remove_worktree_checked(&source, &checked, false).is_err());
    remove_worktree_checked(&source, &checked, true).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_worktree_does_not_mix_active_checkout_edits() {
    let root = std::env::temp_dir().join(format!("pidesk-git-isolation-{}", uuid::Uuid::new_v4()));
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
    let dir = std::env::temp_dir().join(format!("pidesk-git-link-{}", uuid::Uuid::new_v4()));
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

#[test]
fn subdirectory_uses_repo_root_for_file_revert_and_hunks() {
    let root = std::env::temp_dir().join(format!("pidesk-git-subdir-{}", uuid::Uuid::new_v4()));
    let subdir = root.join("workspace");
    std::fs::create_dir_all(&subdir).unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "init", "-q"])
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join("root.txt"), "base\n").unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "-c",
            "user.name=QA",
            "-c",
            "user.email=q@x",
            "commit",
            "-qm",
            "base"
        ])
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join("root.txt"), "changed\n").unwrap();
    let diff = file(&subdir, "root.txt").unwrap();
    assert_eq!(diff.current, "changed\n");
    write_if_unchanged(&subdir, "root.txt", &diff.current_hash, "base\n").unwrap();
    assert!(status(&subdir).unwrap().files.is_empty());
    std::fs::write(root.join("root.txt"), "again\n").unwrap();
    revert_file(&subdir, "root.txt", None).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("root.txt")).unwrap(),
        "base\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn staged_rename_revert_restores_source() {
    let root = std::env::temp_dir().join(format!("pidesk-git-rename-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "init", "-q"])
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join("old.txt"), "base\n").unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "-c",
            "user.name=QA",
            "-c",
            "user.email=q@x",
            "commit",
            "-qm",
            "base"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "mv", "old.txt", "new.txt"])
        .status()
        .unwrap()
        .success());
    revert_file(&root, "new.txt", None).unwrap();
    assert!(!root.join("new.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("old.txt")).unwrap(),
        "base\n"
    );
    assert!(status(&root).unwrap().files.is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn oversized_file_is_hashed_without_loading_payload() {
    let root = std::env::temp_dir().join(format!("pidesk-git-large-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "init", "-q"])
        .status()
        .unwrap()
        .success());
    let path = root.join("large.txt");
    let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789\n";
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    for _ in 0..20_000 {
        std::io::Write::write_all(&mut writer, line.as_bytes()).unwrap();
    }
    std::io::Write::flush(&mut writer).unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "-c",
            "user.name=QA",
            "-c",
            "user.email=q@x",
            "commit",
            "-qm",
            "large"
        ])
        .status()
        .unwrap()
        .success());
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"tail\n")
        .unwrap();
    let changes = status(&root).unwrap();
    assert_eq!(changes.files.len(), 1);
    let reviewed = file(&root, "large.txt").unwrap();
    assert!(reviewed.too_large);
    assert!(reviewed.current.is_empty());
    assert!(!reviewed.current_hash.is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn stress_status_and_large_file_review_stay_bounded() {
    let root = std::env::temp_dir().join(format!("pidesk-git-stress-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "init", "-q"])
        .status()
        .unwrap()
        .success());
    for index in 0..50 {
        std::fs::write(root.join(format!("f{index}.txt")), "base\n").unwrap();
    }
    std::fs::write(root.join("large.txt"), (0..10_000).map(|_| "line of review text 0123456789012345678901234567890123456789012345678901234567890123456789\n").collect::<String>()).unwrap();
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "-c",
            "user.name=QA",
            "-c",
            "user.email=q@x",
            "commit",
            "-qm",
            "base"
        ])
        .status()
        .unwrap()
        .success());
    for index in 0..50 {
        std::fs::write(root.join(format!("f{index}.txt")), "changed\n").unwrap();
    }
    let large = root.join("large.txt");
    let mut append_file = std::fs::OpenOptions::new()
        .append(true)
        .open(&large)
        .unwrap();
    append_file
        .write_all(
            (0..10_000)
                .map(|_| "new line of review text\n")
                .collect::<String>()
                .as_bytes(),
        )
        .unwrap();
    let changes = status(&root).unwrap();
    assert_eq!(changes.files.len(), 51);
    assert_eq!(changes.additions, 10_050);
    let reviewed = file(&root, "large.txt").unwrap();
    assert!(reviewed.too_large);
    assert!(reviewed.current.is_empty());
    let _ = std::fs::remove_dir_all(root);
}
