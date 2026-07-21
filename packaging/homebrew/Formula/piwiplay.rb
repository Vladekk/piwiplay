# Third-party Homebrew formula for piwiplay (NOT an official homebrew-core formula).
#
# This file belongs in a tap repository named `homebrew-piwiplay` so users can:
#   brew tap vladekk/piwiplay
#   brew install piwiplay
#
# piwiplay is Linux-only (it targets PipeWire); the formula is guarded accordingly.
class Piwiplay < Formula
  desc "Console (TUI) DSD audio player over PipeWire"
  homepage "https://github.com/vladekk/piwiplay"
  url "https://github.com/vladekk/piwiplay/archive/refs/tags/v0.2.0.tar.gz"
  # sha256 of the v0.2.0 source tarball (github archive).
  sha256 "d98125f02a8430f80fa68a50eba54f9f5bae8531fb6b51fc356158824c78f525"
  license "MIT"
  head "https://github.com/vladekk/piwiplay.git", branch: "main"

  depends_on "pkg-config" => :build
  depends_on "rust" => :build
  # PipeWire only exists on Linux; refuse to build on macOS.
  depends_on :linux
  depends_on "pipewire"
  # ffmpeg decodes non-DSD formats and powers the DSD->PCM transcode toggle.
  depends_on "ffmpeg"

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/tui")
  end

  def caveats
    <<~EOS
      piwiplay requires a running PipeWire session. Native DSD needs a DSD-capable
      DAC; press `t` to transcode a DSD track to PCM (via ffmpeg) when native is
      unavailable or you want software volume. Non-DSD formats play via ffmpeg
      automatically.
    EOS
  end

  test do
    assert_match "piwiplay #{version}", shell_output("#{bin}/piwiplay --version")
  end
end
