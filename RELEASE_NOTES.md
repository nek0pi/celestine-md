## CelestineMD 0.3.4

### Improvements

- Paste images into notes: clipboard images are saved as timestamped PNGs in a configurable per-vault image folder and displayed inline immediately.
- Resize images with the mouse in both editing and preview. Drag the bottom-right corner, double-click to reset, or set dimensions in Markdown with `![caption|320](image.png)`, `![caption|320x180](image.png)`, or `![[image.png|320]]`.
- Check and uncheck tasks directly in preview, with autosave and animated completion styling.
- Git integration for vault repositories: inspect changes and diffs, commit, pull, and sync from the app.
- Improved graph layout and interaction, including draggable nodes and more responsive settling for larger vaults.
- Empty folders appear in the file tree immediately after creation and can be renamed right away.
- More reliable vault path resolution and image-file URLs on macOS and Windows.
- Canvas boards with drawing, text, note and image cards, and arrows.
- Better table editing and Omarchy theme integration.

### Downloads

| Platform | Archive |
|----------|---------|
| Linux x86_64 | `celestine-linux-x86_64-v0.3.4.tar.gz` |
| macOS Apple Silicon | `celestine-macos-aarch64-v0.3.4.tar.gz` |
| macOS Intel | `celestine-macos-x86_64-v0.3.4.tar.gz` |
| Windows x86_64 | `celestine-windows-x86_64-v0.3.4.zip` |

Archive checksums are provided in `SHA256SUMS`.

### Install or update

Homebrew:

```sh
brew update
brew upgrade nek0pi/celestine/celestine-md
# First install: brew install nek0pi/celestine/celestine-md
```

Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/nek0pi/celestine-md/main/packaging/install-from-release.sh | bash
```

Windows: extract the Windows archive and launch `celestine.exe`.

Close and reopen CelestineMD after updating. Vaults remain plain Markdown files.
