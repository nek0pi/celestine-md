use celestine_core::{Vault, VaultIndex};
use std::fs;
use tempfile::tempdir;

#[test]
fn read_write_and_create_reject_paths_outside_the_vault() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let outside = temp.path().join("outside.md");
    fs::create_dir(&root).unwrap();
    fs::write(&outside, "sentinel").unwrap();
    let mut vault = Vault::open(&root).unwrap();

    assert!(vault.read_note_content("../outside.md").is_err());
    assert!(vault.write_note("../outside.md", "changed").is_err());
    assert!(vault.create_note("../outside-new").is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "sentinel");
    assert!(!temp.path().join("outside-new.md").exists());
    assert!(!temp.path().join("outside-new").exists());

    let absolute = temp.path().join("absolute.md");
    fs::write(&absolute, "absolute sentinel").unwrap();
    assert!(vault.read_note_content(absolute.to_str().unwrap()).is_err());
    assert!(vault
        .write_note(absolute.to_str().unwrap(), "changed")
        .is_err());
    assert!(vault.create_note(absolute.to_str().unwrap()).is_err());
    assert_eq!(fs::read_to_string(&absolute).unwrap(), "absolute sentinel");
}

#[test]
fn folder_and_entry_mutations_reject_outside_paths_without_side_effects() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let outside_dir = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside_dir).unwrap();
    fs::write(outside_dir.join("source.md"), "source sentinel").unwrap();
    fs::write(outside_dir.join("target.md"), "target sentinel").unwrap();
    let mut vault = Vault::open(&root).unwrap();
    fs::write(root.join("inside.md"), "inside sentinel").unwrap();

    assert!(vault.create_folder("../outside/new/deep").is_err());
    assert!(!outside_dir.join("new").exists());
    assert!(vault.delete_entry("../outside/target.md").is_err());
    assert_eq!(
        fs::read_to_string(outside_dir.join("target.md")).unwrap(),
        "target sentinel"
    );
    assert!(vault
        .rename_entry("../outside/source.md", "inside.md")
        .is_err());
    assert!(vault
        .rename_entry("inside.md", "../outside/renamed.md")
        .is_err());
    assert_eq!(
        fs::read_to_string(outside_dir.join("source.md")).unwrap(),
        "source sentinel"
    );
    assert_eq!(
        fs::read_to_string(root.join("inside.md")).unwrap(),
        "inside sentinel"
    );
    assert!(!outside_dir.join("renamed.md").exists());

    let absolute_target = temp.path().join("absolute-folder");
    assert!(vault
        .create_folder(absolute_target.to_str().unwrap())
        .is_err());
    assert!(!absolute_target.exists());
}

#[test]
fn nested_note_operations_and_inside_parent_navigation_work() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("note.md"), "before").unwrap();
    let mut vault = Vault::open(&root).unwrap();

    vault.write_note("Projects/../note.md", "after").unwrap();
    assert_eq!(fs::read_to_string(root.join("note.md")).unwrap(), "after");
    vault.create_note("Projects/Nested/New").unwrap();
    assert!(root.join("Projects/Nested/New.md").is_file());
    vault.create_folder("Projects/Nested/Empty").unwrap();
    vault
        .rename_entry("Projects/Nested/New.md", "Projects/Nested/Renamed.md")
        .unwrap();
    assert!(root.join("Projects/Nested/Renamed.md").is_file());
    vault.delete_entry("Projects/Nested/Renamed.md").unwrap();
    assert!(!root.join("Projects/Nested/Renamed.md").exists());
}

#[cfg(unix)]
#[test]
fn symlinks_to_outside_files_and_directories_are_rejected() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel.md"), "sentinel").unwrap();
    symlink(outside.join("sentinel.md"), root.join("file-link.md")).unwrap();
    symlink(&outside, root.join("dir-link")).unwrap();
    let mut vault = Vault::open(&root).unwrap();

    assert!(vault.read_note_content("file-link.md").is_err());
    assert!(vault.write_note("file-link.md", "changed").is_err());
    assert!(vault.create_note("dir-link/new/note").is_err());
    assert!(vault.write_note("dir-link/new/note.md", "changed").is_err());
    assert!(vault.create_folder("dir-link/new-folder").is_err());
    assert_eq!(
        fs::read_to_string(outside.join("sentinel.md")).unwrap(),
        "sentinel"
    );
    assert!(!outside.join("new").exists());
    assert!(!outside.join("new-folder").exists());
}

#[cfg(unix)]
#[test]
fn dangling_symlinks_cannot_create_files_outside_the_vault() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(
        outside.join("created-by-create.md"),
        root.join("create-link.md"),
    )
    .unwrap();
    symlink(
        outside.join("created-by-write.md"),
        root.join("write-link.md"),
    )
    .unwrap();
    let mut vault = Vault::open(&root).unwrap();

    assert!(vault.create_note("create-link.md").is_err());
    assert!(vault.write_note("write-link.md", "outside").is_err());
    assert!(!outside.join("created-by-create.md").exists());
    assert!(!outside.join("created-by-write.md").exists());
}

#[cfg(unix)]
#[test]
fn symlinks_that_resolve_inside_the_vault_remain_usable() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/note.md"), "before").unwrap();
    symlink(root.join("real/note.md"), root.join("note-link.md")).unwrap();
    symlink(root.join("real"), root.join("dir-link")).unwrap();
    let mut vault = Vault::open(&root).unwrap();

    assert_eq!(vault.read_note_content("note-link.md").unwrap(), "before");
    vault.write_note("note-link.md", "after").unwrap();
    vault.create_note("dir-link/new").unwrap();
    vault.create_folder("dir-link/child").unwrap();
    assert_eq!(
        fs::read_to_string(root.join("real/note.md")).unwrap(),
        "after"
    );
    assert!(root.join("real/new.md").is_file());
    assert!(root.join("real/child").is_dir());
}

#[test]
fn index_upsert_rejects_an_absolute_path_outside_the_vault() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let outside = temp.path().join("outside.md");
    fs::create_dir(&root).unwrap();
    fs::write(&outside, "# Outside\n").unwrap();
    let mut index = VaultIndex::new();

    assert!(index.upsert_file(&root, &outside).is_err());
    assert!(index.is_empty());
}

#[test]
fn metadata_access_accepts_only_single_names_and_checks_the_directory_before_writing() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    fs::create_dir(&root).unwrap();
    let vault = Vault::open(&root).unwrap();

    vault.write_metadata("state.json", "{\"ok\":true}").unwrap();
    assert_eq!(vault.read_metadata("state.json").unwrap(), "{\"ok\":true}");
    for name in ["", ".", "..", "nested/state.json", "nested\\state.json"] {
        assert!(
            vault.write_metadata(name, "bad").is_err(),
            "accepted {name:?}"
        );
        assert!(vault.read_metadata(name).is_err(), "read {name:?}");
    }
    assert!(!root.join("nested").exists());
}

#[cfg(unix)]
#[test]
fn metadata_write_rejects_an_external_celestine_symlink_before_writing() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("state.json"), "sentinel").unwrap();
    symlink(&outside, root.join(".celestine")).unwrap();
    let vault = Vault::open(&root).unwrap();

    assert!(vault.write_metadata("state.json", "changed").is_err());
    assert_eq!(
        fs::read_to_string(outside.join("state.json")).unwrap(),
        "sentinel"
    );
}

#[cfg(unix)]
#[test]
fn index_upsert_rejects_in_vault_symlink_to_outside_markdown() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let outside = temp.path().join("outside.md");
    fs::create_dir(&root).unwrap();
    fs::write(&outside, "# Outside sentinel\n").unwrap();
    symlink(&outside, root.join("linked.md")).unwrap();
    let mut index = VaultIndex::new();

    assert!(index.upsert_file(&root, &root.join("linked.md")).is_err());
    assert!(index.is_empty());
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "# Outside sentinel\n"
    );
}

#[test]
fn native_path_spellings_are_checked_and_nested_backslashes_stay_in_vault() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let outside = temp.path().join("outside.md");
    fs::create_dir(&root).unwrap();
    fs::write(&outside, "sentinel").unwrap();
    let mut vault = Vault::open(&root).unwrap();

    #[cfg(windows)]
    for path in [
        "C:\\outside.md",
        "C:outside.md",
        "\\\\server\\share\\outside.md",
        "..\\outside.md",
    ] {
        assert!(
            vault.write_note(path, "changed").is_err(),
            "accepted {path:?}"
        );
    }
    #[cfg(not(windows))]
    for path in ["\\\\server\\share\\outside.md", "..\\outside.md"] {
        assert!(
            vault.write_note(path, "changed").is_err(),
            "accepted {path:?}"
        );
    }
    assert_eq!(fs::read_to_string(&outside).unwrap(), "sentinel");

    vault.write_note("nested\\safe.md", "inside").unwrap();
    assert_eq!(
        fs::read_to_string(root.join("nested/safe.md")).unwrap(),
        "inside"
    );
    #[cfg(not(windows))]
    {
        vault
            .write_note("Q: plan.md", "drive-like filename")
            .unwrap();
        vault
            .write_note("A: ideas/x.md", "nested drive-like filename")
            .unwrap();
        assert!(root.join("Q: plan.md").is_file());
        assert!(root.join("A: ideas/x.md").is_file());
    }
}

#[cfg(unix)]
#[test]
fn internal_symlink_aliases_share_one_index_identity() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/note.md"), "# Note\n").unwrap();
    symlink(root.join("real/note.md"), root.join("alias.md")).unwrap();
    symlink(root.join("real"), root.join("dir-link")).unwrap();
    let mut vault = Vault::open(&root).unwrap();

    vault.write_note("alias.md", "# Updated\n").unwrap();
    let canonical = vault.index.get_by_rel("real/note.md").unwrap();
    let note_id = canonical.id;
    let alias_path = vault.abs_path("alias.md").unwrap();
    assert_eq!(vault.rel_of(&alias_path).unwrap(), "real/note.md");
    assert_eq!(vault.index.len(), 1);
    assert_eq!(canonical.content, "# Updated\n");
    fs::remove_file(root.join("alias.md")).unwrap();
    let root_path = vault.root().to_path_buf();
    vault.index.remove_path(&root_path, &root.join("alias.md"));
    assert_eq!(vault.index.len(), 1);
    assert_eq!(vault.index.get_by_rel("real/note.md").unwrap().id, note_id);

    vault.write_note("dir-link/new.md", "# New\n").unwrap();
    let new_id = vault.index.get_by_rel("real/new.md").unwrap().id;
    vault
        .index
        .upsert_file(&root, &root.join("real/new.md"))
        .unwrap();
    assert_eq!(vault.index.get_by_rel("real/new.md").unwrap().id, new_id);
    assert_eq!(vault.index.len(), 2);
}

#[cfg(unix)]
#[test]
fn index_accepts_absolute_events_through_a_symlinked_vault_root() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("vault");
    let alias = temp.path().join("vault-alias");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("note.md"), "# Note\n").unwrap();
    symlink(&root, &alias).unwrap();
    let mut index = VaultIndex::new();

    index.upsert_file(&root, &alias.join("note.md")).unwrap();
    assert_eq!(index.len(), 1);
    assert!(index.get_by_rel("note.md").is_some());
}

#[cfg(target_os = "macos")]
#[test]
fn differently_cased_root_event_paths_upsert_and_remove_by_identity() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("Notes");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("note.md"), "# Note\n").unwrap();
    let mut vault = Vault::open(&root).unwrap();
    let event_root = temp.path().join("notes");
    let event_path = event_root.join("note.md");
    let canonical_root = vault.root().to_path_buf();

    if !event_root.exists() {
        assert!(vault
            .index
            .upsert_file(&canonical_root, &event_path)
            .is_err());
        assert_eq!(vault.index.len(), 1);
        return;
    }

    let before = vault.index.get_by_rel("note.md").unwrap().id;
    fs::write(&event_path, "# Updated\n").unwrap();
    vault
        .index
        .upsert_file(&canonical_root, &event_path)
        .unwrap();
    assert_eq!(vault.index.len(), 1);
    assert_eq!(vault.index.get_by_rel("note.md").unwrap().id, before);
    fs::remove_file(&event_path).unwrap();
    vault.index.remove_path(&canonical_root, &event_path);
    assert!(vault.index.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn differently_cased_linux_root_is_not_treated_as_the_same_directory() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("Notes");
    let other = temp.path().join("notes");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&other).unwrap();
    fs::write(root.join("note.md"), "---\ntitle: Note\n---\n# Note\n").unwrap();
    fs::write(other.join("note.md"), "---\ntitle: Other\n---\n# Other\n").unwrap();
    let mut index = VaultIndex::new();
    index.upsert_file(&root, &root.join("note.md")).unwrap();

    assert!(index.upsert_file(&root, &other.join("note.md")).is_err());
    index.remove_path(&root, &other.join("note.md"));
    assert_eq!(index.len(), 1);
    assert_eq!(index.get_by_rel("note.md").unwrap().title, "Note");
}
