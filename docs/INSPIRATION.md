# Inspiration & current shape

CelestineMD is an **independent, open-source Markdown knowledge base** written in Rust (`egui` / `eframe`). It is **inspired by** [Obsidian](https://obsidian.md) — local vaults, `[[wikilinks]]`, graph, backlinks, keyboard-first workflows — but it is **not** a rewrite, clone, or drop-in substitute for Obsidian.

Not affiliated with Obsidian or Dynalist Inc.

---

## Positioning

| Idea from Obsidian-land | How Celestine treats it |
|---|---|
| Plain Markdown folder as vault | Open any folder of `.md` notes; your files stay on disk |
| Wikilinks / backlinks / graph | First-class: `[[links]]`, embeds, outline, backlinks pane, force graph |
| Keyboard-first | Shortcuts on chrome + remappable TOML profiles; file-tree focus mode |
| Themes | CSS custom-property packs → egui tokens (not Electron theme engines) |
| Spatial boards | Celestine canvas: `.canvas` files in the open [JSON Canvas](https://jsoncanvas.org) format, plus Celestine's own ink extension |
| Plugins / Sync | Out of scope by design for now |

Opening a folder that also contains `.obsidian/` is fine: Celestine reads the Markdown, not Obsidian’s app data, plugins, or proprietary formats.

**Do not** unpack or copy proprietary Obsidian binaries, ASAR archives, Electron sources, or UI assets into this repo. Build Celestine as its own software.

---

## What ships today

### Vault & editing
- Open / recent / close vault; multi-tab notes with dirty indicators
- Edit mode with Markdown syntax tint (headings, markers, wikilinks, code)
- Reading preview (headings by level, tables, lists, code, tags, wikilinks)
- Save, new note, find-in-note, vault fuzzy search
- Shortcode → emoji expansion (`:smile:` → 😄); color emoji via system fonts (egui 0.37+)

### Navigation chrome
- **Files** sidebar: rainbow folder colors (theme-seeded), Lucide/flea folder marks, click expand/collapse, double-click / **F2** rename, new folder, delete (empty folders / notes)
- Keyboard file tree: **Ctrl+Tab** toggles files ↔ editor, ↑↓ move, ←→ fold/unfold, Enter open
- **Outline** + **backlinks** on the right; reveal active note in tree
- **Graph** view with folder-tinted clusters
- Sidebars toggle independently; nav back / forward history

### Canvas
A `.canvas` file opens as a pan/zoom board drawn in Celestine's own TUI/pixel style: a pixel dot grid, sharp 1px cards with `┌─ title ─┐` frame titles, hard drop shadows, and orthogonal arrows with chunky heads.

- **Tools** (toolbar or keys): `V` select · `P` ink · `T` text · `N` note · `I` image · `A` arrow · `E` erase; `Mod+Shift+C` makes a new canvas
- **Ink**: a nib-pen brush — strokes are smoothed, thin out when drawn fast, and taper at both ends; colour swatches follow the theme
- **Text cards**: one monospace font, edited in place (double-click)
- **Note cards**: drag a note from the file tree, or create one in place (`N` / right-click → *new note here…*) choosing its folder and name; the card shows a live preview, double-click opens the note
- **Images**: drag from the tree, pick with `I`, drop files from the OS, or paste with `Mod+V` (screenshots, copied images, or files copied in a file manager); outside images are copied into `attachments/` beside the canvas. Pasted plain text becomes a text card
- **Arrows** connect any two objects, including ink drawings; double-click an arrow to label it
- Undo/redo `Mod+Z` / `Mod+Shift+Z`; scroll pans, `Ctrl`+scroll or pinch zooms

**File format.** Cards and arrows are standard JSON Canvas `nodes` / `edges`, so other JSON Canvas tools can read them. Ink lives under a top-level `"celestine"` key (`drawings`, plus `edges` for arrows that touch a drawing), which other readers ignore; unknown fields are kept on save. Renaming or moving a note or image rewrites the file paths inside canvases.

### Themes
Built-in CSS packs (distinct folder/heading palettes per pack):

`obsidian-dark` · `obsidian-light` · `catppuccin` · `tokyo-night` · `gruvbox-dark` · `gruvbox-light` · `nord` · `solarized-dark` · `solarized-light` · `dracula` · `rose-pine` · `everforest-dark` · **`neovoid`** · `one-dark` · `monokai` · `kanagawa` · `ayu-dark` · `oxocarbon` · `high-contrast` · **`terminal`** (Omarchy `colors.toml` when present)

Theme picker: **Mod+T**. Flat flea-like chrome (naked buttons, JetBrains Mono when available).

### Keyboard (defaults)

Mod = **Ctrl** on Linux/Windows, **Cmd** on macOS. Omarchy detection prefers the Omarchy profile (no Super binds).

| Shortcut | Action |
|----------|--------|
| `Mod+O` / `Mod+P` | Quick open / command palette |
| `Mod+G` | Graph |
| `Mod+E` | Edit ↔ preview |
| `Mod+S` / `Mod+N` / `Mod+W` | Save / new note / close tab |
| `Alt+←` / `Alt+→` | Previous / next tab |
| `Ctrl+Tab` | Toggle files ↔ editor |
| `Ctrl+1..9` | Jump tab (Omarchy profile) |
| `F2` / `Mod+Shift+N` / `Delete` | Rename / new folder / delete (tree) |
| `Mod+B` / `Mod+\` | Toggle files / outline sidebars |
| `Mod+[` / `Mod+]` | Navigate back / forward |
| `Mod+Shift+E` | Reveal note in tree |
| `?` / `Mod+/` | Shortcuts cheat sheet |

Remap: copy [`config/keybindings.omarchy.toml`](../config/keybindings.omarchy.toml) (or the Obsidian profile) → `~/.config/celestine/keybindings.toml`.

---

## Architecture (sketch)

```
crates/celestine-core/    vault FS, parse, index, resolve, graph
crates/celestine-theme/   CSS-var packs + Omarchy terminal bridge
crates/celestine-app/     egui shell (tree, editor, preview, graph, overlays)
themes/                   built-in *.css
config/                   default keybinding profiles
assets/                   icon, fonts (outline emoji fallbacks unused when system color emoji works)
```

---

## Deliberately not trying to be Obsidian

Still out of scope unless explicitly designed later:

- Plugin host / community plugin APIs  
- Cloud sync  
- Full Live Preview WYSIWYG as Obsidian does it  
- Loading arbitrary community `theme.css` engines / snippets ecosystems  

Celestine stays **ultralight**: native, local, keyboard-honest, Markdown-native — inspired by Obsidian’s *workflow*, not its surface area.
