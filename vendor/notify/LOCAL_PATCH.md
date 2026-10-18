# Local patch for notify 7.0.0

This directory is the published `notify` 7.0.0 crate from
<https://crates.io/crates/notify/7.0.0>. Its upstream source is
<https://github.com/notify-rs/notify/tree/a18addef1d4dff6f9a54d9234275d2764dffe72a/notify>.
The original crates.io archive checksum recorded in `Cargo.lock` is
`c533b4c39709f9ba5005d8002048266593c1cfaf3c5f0739d5b8ab0c6c504009`.
`Cargo.toml.orig` preserves the upstream workspace manifest, and
`Cargo.toml.upstream` preserves the exact registry-published manifest.
The local `Cargo.toml` removes the unused `insta`, `nix`, and `serde_json`
dev dependencies so its focused regression test can run without adding
unneeded dependencies to the workspace lockfile. Its package version,
features, and production dependencies are unchanged.

The only production change is in `src/inotify.rs`: after reporting a hard
`read_events` error, `handle_inotify` breaks out of its drain loop. This lets
the outer event loop process queued messages, including `Shutdown`, instead
of retrying the same failing read indefinitely. A Linux regression test in
that file forces a real read error and checks that the handler runs once and
the queued shutdown is processed.

The upstream integration test canonicalizes its temporary root before comparing
notification paths. This preserves the path assertion on macOS, where `/var`
and `/private/var` name the same directory but FSEvents reports the latter.
It also waits up to one fixed ten-second deadline for the expected file event,
retaining the exact file-path assertion while allowing unrelated parent events
to arrive first. The fixture queues one parent event to exercise that ordering;
the expected file event still comes from the real platform watcher.
