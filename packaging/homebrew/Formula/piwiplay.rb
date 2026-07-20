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
  url "https://github.com/vladekk/piwiplay/archive/refs/tags/v0.1.0.tar.gz"
  # sha256 of the v0.1.0 source tarball (github archive).
  sha256 "cf739401b8b9b9db135a781243859b0a9a0f353aafac000dafcce553d3dc0c34"
  license "MIT"
  head "https://github.com/vladekk/piwiplay.git", branch: "main"

  depends_on "pkg-config" => :build
  depends_on "rust" => :build
  # PipeWire only exists on Linux; refuse to build on macOS.
  depends_on :linux
  depends_on "pipewire"

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/tui")
  end

  def caveats
    <<~EOS
      piwiplay requires a running PipeWire session and a DAC that accepts native
      DSD. On sinks whose active profile exposes no DSD format, playback will
      report an error rather than convert (DoP/transcoding arrive in v2).
    EOS
  end

  test do
    assert_match "piwiplay #{version}", shell_output("#{bin}/piwiplay --version")
  end
end
