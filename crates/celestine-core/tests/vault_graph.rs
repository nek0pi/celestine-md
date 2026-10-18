use celestine_core::{resolve_link, ResolvedLink, Vault};
use std::path::PathBuf;

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
    use std::time::Duration;

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
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut observed = false;
        while std::time::Instant::now() < deadline {
            if vault.poll_fs() {
                observed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(observed, "watcher did not report the symlink creation");
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

fn wait_for(vault: &mut Vault, timeout: std::time::Duration, predicate: impl Fn(&Vault) -> bool) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        vault.poll_fs();
        if predicate(vault) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        predicate(vault),
        "filesystem watcher state did not converge"
    );
}
