# Homebrew formula for a tap (e.g. codeoba-org/homebrew-tap, file Formula/shhoba.rb).
#
# After tagging a release, replace VERSION and the four sha256 values with the
# contents of the .sha256 files attached to the GitHub release.
class Shhoba < Formula
  desc "TUI and CLI for managing SSH servers, tunnels and file transfers"
  homepage "https://github.com/codeoba-org/sshoba"
  version "1.0.0"
  license "MIT"

  base = "https://github.com/codeoba-org/sshoba/releases/download/v#{version}"

  on_macos do
    on_arm do
      url "#{base}/shhoba-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_AARCH64_APPLE_DARWIN"
    end
    on_intel do
      url "#{base}/shhoba-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_X86_64_APPLE_DARWIN"
    end
  end

  on_linux do
    on_arm do
      url "#{base}/shhoba-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_AARCH64_UNKNOWN_LINUX_GNU"
    end
    on_intel do
      url "#{base}/shhoba-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_X86_64_UNKNOWN_LINUX_GNU"
    end
  end

  def install
    bin.install "shhoba"
    generate_completions_from_executable(bin/"shhoba", "completions")
  end

  test do
    ENV["SHHOBA_CONFIG_DIR"] = testpath/"cfg"
    ENV["XDG_STATE_HOME"] = testpath/"state"
    system bin/"shhoba", "add", "web", "me@192.0.2.1"
    assert_match "192.0.2.1", shell_output("#{bin}/shhoba ls")
  end
end
