# Copy of Formula/shhoba.rb in https://github.com/codeoba-org/homebrew-tap (the tap is
# the source of truth). For a new release: push the vX.Y.Z tag, wait for the Release
# workflow, then in the tap replace the version in all four URLs and each sha256 with
# the contents of the matching .tar.gz.sha256 asset, and run
# `brew audit --strict codeoba-org/tap/shhoba`.
class Shhoba < Formula
  desc "TUI and CLI for managing SSH servers, tunnels and file transfers"
  homepage "https://github.com/codeoba-org/sshoba"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/codeoba-org/sshoba/releases/download/v1.0.0/shhoba-v1.0.0-aarch64-apple-darwin.tar.gz"
      sha256 "adb9a9dd127fbffdfb6bacf395ed709c45115335c49128f7eacd7a8bf5a5185d"
    end
    on_intel do
      url "https://github.com/codeoba-org/sshoba/releases/download/v1.0.0/shhoba-v1.0.0-x86_64-apple-darwin.tar.gz"
      sha256 "d6e89b86e843058199a69789ffbf862e8f7a3b30de2fc984a9704e2ef7d53eb4"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/codeoba-org/sshoba/releases/download/v1.0.0/shhoba-v1.0.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "47d8bcde3c562afebecd132879c004211455ceba21ad369d76b0ea1f646e994f"
    end
    on_intel do
      url "https://github.com/codeoba-org/sshoba/releases/download/v1.0.0/shhoba-v1.0.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0919b3813563de7b02feeee874fbfa1b842a459c434e391988a64705c0ebba68"
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
