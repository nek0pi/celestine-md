use celestine_core::{resolve_link, ResolvedLink, Vault};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[test]
fn fixture_vault_links_and_backlinks() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/vaults/minimal");
    let vault = Vault::open(&root).expect("open fixture");
    assert!(vault.index.len() >= 2);

    let home = vault.index.get_by_rel("Home.md").expect("Home.md");
    let alpha = vault.index.get_by_rel("Projects/Alpha.md").expect("Alpha");

    match resolve_link(&vault.index, &home.rel, "Projects/Alpha") {
        ResolvedLink::Note(id) => assert_eq!(id, alpha.id),
        other => panic!("expected note, got {other:?}"),
    }

    let backs = vault.graph.backlinks(&vault.index, home.id);
    assert!(backs.contains(&alpha.id), "Alpha should backlink Home");

    let edges = vault.graph.all_edges();
    assert!(
        edges
            .iter()
            .any(|(a, b)| (*a == home.id && *b == alpha.id) || (*a == alpha.id && *b == home.id)),
        "expected Home–Alpha edge"
    );
}

#[test]
fn folder_rename_reindexes_notes_and_preserves_graph_and_backlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    std::fs::create_dir_all(root.join("Old")).unwrap();
    std::fs::write(root.join("Old/Alpha.md"), "# Alpha\n\n[[Beta]]\n").unwrap();
    std::fs::write(root.join("Old/Beta.md"), "# Beta\n\n[[Alpha]]\n").unwrap();
    std::fs::write(root.join("Home.md"), "# Home\n\n[[Alpha]]\n").unwrap();
    let mut vault = Vault::open(&root).unwrap();
    let home_id = vault.index.get_by_rel("Home.md").unwrap().id;

    vault.rename_entry("Old", "New").unwrap();

    assert!(vault.index.get_by_rel("Old/Alpha.md").is_none());
    assert!(vault.index.get_by_rel("Old/Beta.md").is_none());
    let alpha_id = vault.index.get_by_rel("New/Alpha.md").unwrap().id;
    let beta_id = vault.index.get_by_rel("New/Beta.md").unwrap().id;
    assert!(vault
        .graph
        .backlinks(&vault.index, alpha_id)
        .contains(&home_id));
    assert!(vault
        .graph
        .backlinks(&vault.index, alpha_id)
        .contains(&beta_id));
    assert!(vault
        .graph
        .backlinks(&vault.index, beta_id)
        .contains(&alpha_id));
}

#[test]
fn watcher_poll_indexes_create_modify_and_remove_events() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("Target.md"), "# Target\n").unwrap();
    let outside = temp.path().join("outside.md");
    std::fs::write(&outside, "# Outside\n").unwrap();
    let mut vault = Vault::open(&root).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(&outside, root.join("external.md")).unwrap();
        // Drain watcher events; escaping symlinks must not enter the index, and
        // rejecting them is not itself an index/sidebar change under idle reconcile.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let _ = vault.poll_fs();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(vault.index.get_by_rel("external.md").is_none());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "# Outside\n");
    }

    std::fs::write(root.join("Source.md"), "# Source\n\n[[Target]]\n").unwrap();
    wait_for(&mut vault, Duration::from_secs(3), |v| {
        v.index.get_by_rel("Source.md").is_some()
            && v.graph
                .backlinks(&v.index, v.index.get_by_rel("Target.md").unwrap().id)
                .contains(&v.index.get_by_rel("Source.md").unwrap().id)
    });

    std::fs::write(root.join("Source.md"), "# Source\n\n[[Missing]]\n").unwrap();
    wait_for(&mut vault, Duration::from_secs(3), |v| {
        v.index.get_by_rel("Source.md").unwrap().wikilinks[0].target == "Missing"
            && v.graph
                .backlinks(&v.index, v.index.get_by_rel("Target.md").unwrap().id)
                .is_empty()
    });

    std::fs::remove_file(root.join("Source.md")).unwrap();
    wait_for(&mut vault, Duration::from_secs(3), |v| {
        v.index.get_by_rel("Source.md").is_none()
    });
    assert!(vault.index.get_by_rel("outside.md").is_none());
}

fn wait_for(vault: &mut Vault, timeout: Duration, predicate: impl Fn(&Vault) -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        vault.poll_fs();
        if predicate(vault) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        predicate(vault),
        "filesystem watcher state did not converge"
    );
}

fn wait_for_refresh(
    vault: &mut Vault,
    wakes: &Receiver<()>,
    change: &str,
    ready: impl Fn(&Vault) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        wakes.recv_timeout(remaining).unwrap_or_else(|error| {
            panic!("timed out waiting for watcher wake after {change}: {error}");
        });
        if vault.poll_fs() && ready(vault) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "watcher did not refresh after {change}"
        );
    }
}

#[test]
fn external_note_image_and_directory_changes_wake_and_refresh() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Folder/nested")).unwrap();
    std::fs::write(dir.path().join("Folder/nested/Child.md"), "[[Home]]").unwrap();
    std::fs::write(dir.path().join("Home.md"), "# Home").unwrap();
    let (wake_tx, wakes) = mpsc::channel();
    let mut vault = Vault::open_with_wakeup(dir.path(), move || {
        let _ = wake_tx.send(());
    })
    .unwrap();
    assert!(
        vault.watcher_error().is_none(),
        "{:?}",
        vault.watcher_error()
    );
    assert!(
        vault.next_fs_poll().is_none(),
        "healthy watcher must stay timer-free"
    );
    let root = vault.root().to_path_buf();
    let home_id = vault.index.get_by_rel("Home.md").unwrap().id;

    std::fs::write(root.join("New.md"), "[[Home]]").unwrap();
    wait_for_refresh(&mut vault, &wakes, "note creation", |vault| {
        vault.index.get_by_rel("New.md").is_some()
    });
    let new_id = vault.index.get_by_rel("New.md").unwrap().id;
    assert!(vault.graph.neighbors(home_id).contains(&new_id));

    std::fs::write(root.join("New.md"), "# Edited").unwrap();
    wait_for_refresh(&mut vault, &wakes, "note edit", |vault| {
        vault
            .index
            .get_by_rel("New.md")
            .is_some_and(|note| note.content == "# Edited")
    });
    assert_eq!(vault.index.get_by_rel("New.md").unwrap().id, new_id);
    assert!(!vault.graph.neighbors(home_id).contains(&new_id));

    std::fs::rename(root.join("New.md"), root.join("Renamed.md")).unwrap();
    wait_for_refresh(&mut vault, &wakes, "note rename", |vault| {
        vault.index.get_by_rel("New.md").is_none()
            && vault.index.get_by_rel("Renamed.md").is_some()
    });
    std::fs::remove_file(root.join("Renamed.md")).unwrap();
    wait_for_refresh(&mut vault, &wakes, "note deletion", |vault| {
        vault.index.get_by_rel("Renamed.md").is_none()
    });

    std::fs::write(root.join("picture.png"), b"image inventory fixture").unwrap();
    wait_for_refresh(&mut vault, &wakes, "image creation", |vault| {
        vault.list_image_rel_paths() == ["picture.png"]
    });
    std::fs::rename(root.join("picture.png"), root.join("renamed.png")).unwrap();
    wait_for_refresh(&mut vault, &wakes, "image rename", |vault| {
        vault.list_image_rel_paths() == ["renamed.png"]
    });
    std::fs::remove_file(root.join("renamed.png")).unwrap();
    wait_for_refresh(&mut vault, &wakes, "image deletion", |vault| {
        vault.list_image_rel_paths().is_empty()
    });

    std::fs::rename(root.join("Folder"), root.join("Moved")).unwrap();
    wait_for_refresh(&mut vault, &wakes, "directory rename", |vault| {
        vault.index.get_by_rel("Folder/nested/Child.md").is_none()
            && vault.index.get_by_rel("Moved/nested/Child.md").is_some()
    });
    std::fs::remove_dir_all(root.join("Moved")).unwrap();
    wait_for_refresh(&mut vault, &wakes, "directory deletion", |vault| {
        vault.list_md_rel_paths() == ["Home.md"]
    });
    assert_eq!(vault.index.get_by_rel("Home.md").unwrap().id, home_id);
    assert!(vault.graph.neighbors(home_id).is_empty());
}
