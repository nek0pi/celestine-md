# Homebrew formula for CelestineMD (prebuilt binaries from GitHub Releases).
#
# Tap: https://github.com/nek0pi/homebrew-celestine
#
# Users:
#   brew tap nek0pi/celestine && brew install celestine-md
#   brew install --HEAD nek0pi/celestine/celestine-md   # build from main

class CelestineMd < Formula
  desc "Ultralight Obsidian-inspired Markdown notes (Rust / egui)"
  homepage "https://github.com/nek0pi/celestine-md"
  license "MIT"
  version "0.3.1"

  on_macos do
    on_arm do
      url "https://github.com/nek0pi/celestine-md/releases/download/v0.3.1/celestine-macos-aarch64-v0.3.1.tar.gz"
      sha256 "3191ec43051b0fd9b1a060a84d1b7bdf73937466647648619912d2375918bec1"
    end
    on_intel do
      url "https://github.com/nek0pi/celestine-md/releases/download/v0.3.1/celestine-macos-x86_64-v0.3.1.tar.gz"
      sha256 "296401c43db5bf7cd227f0c7f6d82cb67ed69688b9aa14116ca1cb26904a7d6c"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/nek0pi/celestine-md/releases/download/v0.3.1/celestine-linux-x86_64-v0.3.1.tar.gz"
      sha256 "e7f6ee54dda6f6b42e73cd15baab2102cbe9e93bab0406a6a9fff91b50292026"
    end
  end

  head do
    url "https://github.com/nek0pi/celestine-md.git", branch: "main"
    depends_on "rust" => :build
    on_linux do
      depends_on "pkgconf" => :build
    end
  end

  def install
    if build.head?
      system "cargo", "build", "--release", "-p", "celestine-app"
      bin.install "target/release/celestine"
      (pkgshare/"themes").install Dir["themes/*.css"]
      (pkgshare/"config").install Dir["config/keybindings.*.toml"]
    else
      # Release tarball layout: bin/celestine + share/...
      bin.install "bin/celestine"
      (pkgshare/"themes").install Dir["share/celestine/themes/*.css"] if Dir.exist?("share/celestine/themes")
      (pkgshare/"config").install Dir["share/celestine/config/*.toml"] if Dir.exist?("share/celestine/config")
    end
  end

  def caveats
    <<~EOS
      Themes ship under #{pkgshare}/themes
      Optional: copy a keybinding profile to ~/.config/celestine/keybindings.toml
      Launch: celestine ~/Notes
    EOS
  end

  test do
    assert_predicate bin/"celestine", :exist?
  end
end
