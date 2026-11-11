class Retrace < Formula
  desc "Record/replay reverse debugger for Apple Silicon"
  homepage "https://github.com/NMitchem/retrace"
  license any_of: ["MIT", "Apache-2.0"]
  head "https://github.com/NMitchem/retrace.git", branch: "main"

  # The stable stanza needs the tarball of the tag v0.1.0, which exists only after the operator pushes
  # that tag. Until then the formula is HEAD-only: `brew install nmitchem/retrace/retrace` without --HEAD
  # fails with Homebrew's own message, "is a HEAD-only formula". (A short name needs the formula trusted first
  # under Homebrew 6 and later, so the fully qualified name is the one to tell a user.)
  # After the tag, fill in the checksum (curl -L <url> | shasum -a 256) and uncomment these two lines.
  # url "https://github.com/NMitchem/retrace/archive/refs/tags/v0.1.0.tar.gz"
  # sha256 "FILL-IN-AFTER-THE-TAG"

  depends_on "rustup" => :build
  depends_on arch: :arm64
  depends_on macos: :tahoe

  def install
    # The pinned toolchain (rust-toolchain.toml) through keg-only rustup, kept inside the build
    # directory so nothing is linked over the user's own cargo.
    ENV["RUSTUP_HOME"] = buildpath/".rustup"
    ENV["CARGO_HOME"] = buildpath/".cargo"
    ENV.prepend_path "PATH", buildpath/".cargo/bin"
    ENV.prepend_path "PATH", formula_opt_bin("rustup")
    # The version is read from that file, so a pin bump cannot leave the formula on another toolchain.
    toolchain = File.read("rust-toolchain.toml")[/^channel\s*=\s*"([^"]+)"/, 1]
    odie "no channel in rust-toolchain.toml" if toolchain.nil?
    system formula_opt_bin("rustup")/"rustup", "toolchain", "install", toolchain,
           "--profile", "minimal", "-t", "aarch64-apple-darwin"
    system formula_opt_bin("rustup")/"rustup", "default", toolchain
    system "cargo", "install", "--locked", "--path", "crates/retrace", "--root", prefix
    # Every binary that touches the hypervisor needs this entitlement. Signing after cargo's link
    # step keeps it (t0 measurement 1).
    system "codesign", "-s", "-", "-f", "--entitlements", "retrace.entitlements", bin/"retrace"
    pkgshare.install "retrace.entitlements"
    (pkgshare/"examples").install "crates/retrace-guest/c/crashy.c"
    (pkgshare/"skills/retrace").install "plugin/skills/retrace/SKILL.md"
    (pkgshare/"examples").install "crates/retrace-guest/agent/ledger"
  end

  def caveats
    <<~EOS
      retrace's lldb front door (retrace lldb) needs the Command Line Tools:
        xcode-select --install

      The README's example program is installed here:
        #{pkgshare}/examples/crashy.c

      The Claude Code skill that teaches an agent to use retrace, and a Rust example crate with a
      failing test to practise on, are installed here:
        #{pkgshare}/skills/retrace/SKILL.md
        #{pkgshare}/examples/ledger
      Copy the skill into ~/.claude/skills/retrace/, or install it as a plugin:
        claude plugin marketplace add NMitchem/retrace
        claude plugin install retrace@retrace

      The first run of a freshly signed binary can pause for a few seconds in macOS's
      code-signing check.
    EOS
  end

  test do
    assert_match "retrace 0.1.0", shell_output("#{bin}/retrace --version")
    assert_match "com.apple.security.hypervisor",
                 shell_output("codesign -d --entitlements - #{bin}/retrace 2>&1")
    (testpath/"three.c").write "int main(void){return 3;}\n"
    system ENV.cc, "-o", "three", "three.c"
    shell_output("#{bin}/retrace record -o three.retrace ./three", 3)
    assert_path_exists testpath/"three.retrace"
  end
end
