# Reactive idle rendering

Celestine should settle to near-zero CPU use when a visible editor has no work to
perform. The app should request frames for input, file-system events, state
changes, and active animation, then sleep while idle. A visible caret blink and
other active animations are legitimate work; a fully settled graph and overlays
should not keep the UI busy.

Check watching by creating, editing, renaming, and deleting Markdown files and
images, and by adding or removing directories while the app is idle. Updates
should appear without keyboard input. Reported watcher setup or runtime errors
enable disk reconciliation every two seconds. The failed watcher is retired;
there is no automatic registration retry, so reopen the vault to restore watching.

Some backends can silently fail to register a newly created subtree, including
when Linux exhausts its watch limit. No error reaches the app in that case, so
the fallback does not start and changes in that subtree can be missed. Healthy
watchers have no periodic safety scan. After resolving the resource limit,
reopen the vault to register its watches again. Watcher recovery and event
delivery are separate checks from idle CPU.

## Reproducible CPU measurement

1. Use the same visible 1280×800 window and a copied 27-note vault, with the
   same note open in edit view and both sidebars visible. Keep focus outside
   the text editor so no caret blinks, and wait for startup work to settle.
   Do not type or interact during a sample.
2. Measure process CPU time and monotonic wall time over a 20-second interval:
   `100 × Δprocess CPU seconds / Δmonotonic seconds`. Report the result as
   percent of one core; 100% means one core was fully occupied.
3. Take three 20-second samples under the same conditions and report all three
   results and their median. Record the OS, build type, window visibility, and
   whether the app was unobscured.
4. Profile separately to identify remaining work. Repeat with the window
   occluded or blocked as a distinct condition; do not compare that result with
   the visible idle measurement.

## Initial PR idle result (`c5e0662`)

Measured on macOS 26.5.1 (Apple silicon), using the upstream `80936b1` baseline
and the initial PR revision, both optimized with release debug information. Each
sample lasted 20 seconds; updates were disabled with `CELESTINE_NO_UPDATE=1`.
The copied vault contained 27 Markdown notes totaling 56,670 bytes.

| Build | CPU samples (% of one core) | Median |
| --- | --- | --- |
| Upstream baseline | 13.22, 12.66, 12.72 | 12.72% |
| Reactive rendering | 0.00, 0.00, 0.00 | 0.00% |

Readings near zero are the measurement floor, not proof of zero CPU work. This
is a small-vault, visible-idle result; it does not predict large-vault startup,
active editing, or degraded watcher-mode costs. Re-run the same workload when
changing the watcher or frame scheduler.

## Reconciliation benchmark after review fixes

Run the ignored synthetic workload separately from normal workspace tests:

```sh
cargo test -p celestine-core --release --locked \
  large_vault_reconciliation_measurement -- --ignored --nocapture
```

The macOS release-build workload contained 3,000 Markdown notes (3,316,890
bytes), 300 image inventory entries, and 3,000 hidden `.git` files. Five warm
samples measured the synchronous reconciliation call, excluding event-delivery
latency, note generation, rendering, and startup.

| Operation | Samples (ms) | Median | Note reads per sample |
| --- | --- | --- | --- |
| Unchanged fallback scan | 7.886, 7.609, 7.430, 7.290, 7.663 | 7.609 ms | 0 |
| Atomic-save rename event | 9.311, 8.942, 9.502, 9.369, 9.773 | 9.369 ms | 1 |

A parent-directory `Modify` event performed zero full scans and zero note reads,
returned no content change, and took 0.005 ms in one observation. The event is
injected to test its handling independently of the host OS's notification shape.

These are warm synthetic reconciliation timings, not UI latency or CPU usage,
and no equivalent pre-fix large-vault baseline was measured. Fallback still
checks metadata across visible entries; it skips unchanged note reads/parsing
and rebuilds the graph only when indexed notes actually change. Hidden trees
are pruned before traversal. Explicit mutation and lost-event signals bypass
metadata-only assumptions so same-size edits can still be discovered.

Fallback scans use file size and modification time to detect unwitnessed note
edits. An edit that preserves both, with no delivered mutation or rescan signal,
requires reopening the vault to refresh that content.
