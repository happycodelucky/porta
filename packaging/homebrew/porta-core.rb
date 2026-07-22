# Reference formula for submission to Homebrew/homebrew-core.
#
# This is deliberately NOT the tap formula. The tap formula (porta.rb.in)
# installs a prebuilt release archive, which homebrew-core does not accept:
# core formulae must build from source, and Homebrew's own CI then produces
# the bottles users actually download.
#
# This file lives here so it can be audited alongside the code it packages.
# The formula that ships is the copy in a homebrew-core fork; after the first
# release lands there, Homebrew's bots handle version bumps.
#
# Before submitting, fill in the source tarball checksum:
#
#   curl -fsSLo porta.tar.gz \
#     https://github.com/happycodelucky/porta/archive/refs/tags/v0.9.0.tar.gz
#   shasum -a 256 porta.tar.gz
#
# Then validate locally, from a homebrew-core checkout:
#
#   brew install --build-from-source ./Formula/p/porta.rb
#   brew test porta
#   brew audit --strict --new --online porta
#   brew style --fix porta
class Porta < Formula
  desc "Reserve and lease local TCP ports for parallel worktrees and dev servers"
  homepage "https://github.com/happycodelucky/porta"
  url "https://github.com/happycodelucky/porta/archive/refs/tags/v0.9.0.tar.gz"
  sha256 "@SHA256_SOURCE@"
  license "MIT"
  head "https://github.com/happycodelucky/porta.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    ENV["PORTA_HOME"] = testpath/"state"

    assert_match version.to_s, shell_output("#{bin}/porta --version")

    # Registry-only paths, so these hold even where the sandbox restricts
    # binding: an empty registry, then a typed configuration read.
    assert_match "No reservations or leases.", shell_output("#{bin}/porta list")
    assert_equal "55000", shell_output("#{bin}/porta config get default_port").strip

    # The real behaviour: reserve a keyed port for a directory and read it back.
    # Allocation probes sockets, so this also proves the binary can bind.
    system bin/"porta", "reserve", "--key", "web", testpath
    assert_match(/\A\d+\n\z/, shell_output("#{bin}/porta get --key web #{testpath}"))
  end
end
