# Cutting a CelestineMD release

## Versioning & in-app notices

CelestineMD uses `MAJOR.MINOR.PATCH` (e.g. `0.2.0`):

| Bump | Use for | In-app “update available” |
|------|---------|---------------------------|
| **MINOR** (or MAJOR) | Feature / bigger functionality | Yes — banner links to GitHub Releases |
| **PATCH** | Bugfixes / small improvements | No |

The app compares the running build to the latest GitHub release and notifies only when the feature line (`MAJOR.MINOR`) advances. Set `CELESTINE_NO_UPDATE=1` to disable the check.

## What this repo automates

GitHub Actions (`.github/workflows/release.yml`) builds **Linux**, **macOS** (Intel + Apple Silicon), and **Windows** archives when you push a version tag (`v0.1.0`, …).

The workflow verifies that the tag matches the Cargo version and publishes only
after all four builds succeed. It includes `SHA256SUMS` and uses
`RELEASE_NOTES.md` for the release description. Update those notes before tagging.
For a manual rerun, supply the existing version tag in the workflow's `tag` input.

## What you must do (accounts I can’t use for you)

| Channel | Ready in-repo? | Needs your action |
|---------|----------------|-------------------|
| **GitHub Releases** (binaries) | ✅ workflow + `v0.2.0` live | Push tags when cutting releases |
| **Linux one-liner** | ✅ `packaging/install-from-release.sh` | None after script is on `main` |
| **Homebrew** | ✅ tap live (`nek0pi/homebrew-celestine`) | Bump formula checksums on each release |
| **Arch AUR** | ✅ `PKGBUILD` + `.SRCINFO` in-repo | One-time: AUR account + SSH key, then push package |
| **Flathub / Snap** | ❌ not set up | Separate packaging + review queues |

I cannot publish to Homebrew core, AUR, or Flathub without your credentials and review processes.

---

## 1. First release checklist

```bash
# From a clean tree with release-ready code committed & pushed to main:
git tag -a v0.1.0 -m "CelestineMD v0.1.0"
git push origin main
git push origin v0.1.0
```

Watch **Actions → Release**. When green, the GitHub Release page will have:

- `celestine-linux-x86_64-v0.1.0.tar.gz`
- `celestine-macos-aarch64-v0.1.0.tar.gz`
- `celestine-macos-x86_64-v0.1.0.tar.gz`
- `celestine-windows-x86_64-v0.1.0.zip`

### Local Linux tarball (optional, without CI)

```bash
./packaging/build-linux-dist.sh 0.1.0
# → dist/celestine-linux-x86_64-v0.1.0.tar.gz
./dist/celestine-linux-x86_64/install.sh   # installs to ~/.local
```

---

## 2. Homebrew (your tap)

The formula installs **prebuilt** macOS/Linux release archives (fast). `--HEAD` still builds from source.

1. Re-auth if needed: `gh auth login -h github.com`
2. Create the tap and push the formula:

```bash
gh repo create nek0pi/homebrew-celestine --public --clone
cd homebrew-celestine
mkdir -p Formula
cp /path/to/celestine-md/packaging/homebrew/celestine-md.rb Formula/
git add Formula/celestine-md.rb
git commit -m "celestine-md 0.2.0"
git push -u origin main
```

3. Users install:

```bash
brew tap nek0pi/celestine
brew install celestine-md
```

### Bumping the formula on each release

After CI uploads the new archives, refresh checksums and `version` in the formula:

```bash
# macOS Apple Silicon / Intel / Linux x86_64 release tarballs
curl -sL "https://github.com/nek0pi/celestine-md/releases/download/vX.Y.Z/celestine-macos-aarch64-vX.Y.Z.tar.gz" | shasum -a 256
curl -sL "https://github.com/nek0pi/celestine-md/releases/download/vX.Y.Z/celestine-macos-x86_64-vX.Y.Z.tar.gz" | shasum -a 256
curl -sL "https://github.com/nek0pi/celestine-md/releases/download/vX.Y.Z/celestine-linux-x86_64-vX.Y.Z.tar.gz" | shasum -a 256
```

Commit the updated `Formula/celestine-md.rb` in `homebrew-celestine`.

Getting into **homebrew-core** is a separate PR review and usually wants notable usage — start with your tap.

---

## 3. Arch Linux (AUR)

1. Register on [aur.archlinux.org](https://aur.archlinux.org/)
2. `git clone ssh://aur@aur.archlinux.org/celestine-md.git`
3. Copy `packaging/aur/PKGBUILD`, bump `pkgver` / `sha256sums` if needed, run:

```bash
# source tarball checksum (AUR builds from tag source)
curl -sL "https://github.com/nek0pi/celestine-md/archive/refs/tags/v0.2.0.tar.gz" | sha256sum
makepkg --printsrcinfo > .SRCINFO
makepkg -si   # test locally
git add PKGBUILD .SRCINFO && git commit -m "celestine-md 0.2.0-1" && git push
```

Users: `yay -S celestine-md` / `paru -S celestine-md`.

Optional later: `celestine-md-bin` PKGBUILD that downloads the GitHub release binary (faster installs).

---

## 4. End-user install summary

| OS | Easiest path |
|----|----------------|
| **Linux** | `curl …/install-from-release.sh \| bash`, or AUR when published |
| **macOS** | `brew tap nek0pi/celestine && brew install celestine-md` (after tap is public) |
| **Windows** | Download `.zip`, run `celestine.exe` (SmartScreen may warn on first unsigned builds) |

Code-sign / notarize for macOS and Authenticode for Windows are **not** included yet (needs Apple/Microsoft developer accounts + certificates).
