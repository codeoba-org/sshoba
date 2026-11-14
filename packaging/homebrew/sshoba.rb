# Copy of Formula/sshoba.rb in https://github.com/codeoba-org/homebrew-tap (the tap is
# the source of truth). For a new release: push the vX.Y.Z tag, wait for the Release
# workflow, then in the tap replace the version in all four URLs and each sha256 with
# the contents of the matching .tar.gz.sha256 asset, and run
# `brew audit --strict codeoba-org/tap/sshoba`.
class Sshoba < Formula
  desc "TUI and CLI for managing SSH servers, tunnels and file transfers"
  homepage "https://github.com/codeoba-org/sshoba"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/codeoba-org/sshoba/releases/download/v1.1.0/sshoba-v1.1.0-aarch64-apple-darwin.tar.gz"
      sha256 "671e657410c8a346c41c6f8d1b8041da0e35edf653aa1cc15238f787e91a0c4a"
    end
    on_intel do
      url "https://github.com/codeoba-org/sshoba/releases/download/v1.1.0/sshoba-v1.1.0-x86_64-apple-darwin.tar.gz"
      sha256 "4bf5e6c9235e05a2a33291ec75b61b38a69e15c77d6ac72e2fa7c85a1a57f1e8"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/codeoba-org/sshoba/releases/download/v1.1.0/sshoba-v1.1.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "af3e257b40236e87f7c1eae7db6351928e3f750455fa6a4848b8ce0ad3bf9cf3"
    end
    on_intel do
      url "https://github.com/codeoba-org/sshoba/releases/download/v1.1.0/sshoba-v1.1.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "502653e3b3c035508faef13216c8f033e2543abad406884fec8f37ec44ef1472"
    end
  end

  def install
    bin.install "sshoba"
    generate_completions_from_executable(bin/"sshoba", "completions")
  end

  test do
    ENV["SSHOBA_CONFIG_DIR"] = testpath/"cfg"
    ENV["XDG_STATE_HOME"] = testpath/"state"
    system bin/"sshoba", "add", "web", "me@192.0.2.1"
    assert_match "192.0.2.1", shell_output("#{bin}/sshoba ls")
  end
end
