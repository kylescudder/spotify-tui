# Release operations

The repository treats packaging as a tested product surface. Pull requests run
Rust checks on Linux, macOS, and Windows, validate the locked Nix flake, and test
the POSIX and PowerShell installers. A semantic version tag assembles and tests
every release path before publishing anything.

## One-time repository setup

1. Confirm `kylescudder/spotify-tui` is the protected canonical repository and
   this checkout points `origin` at it. If a different canonical repository is
   chosen, update the Cargo, Nix, and README metadata; generated installers use
   the actual Actions repository automatically.
2. Protect `main` and require the CI jobs before merging. Do not use
   `pull_request_target`; untrusted pull requests receive read-only repository
   permissions and no release secrets.
3. The release workflow targets the existing `kylescudder/homebrew-tap`
   repository. Its Homebrew name is `kylescudder/tap`, and release formulae are
   placed in its existing `Formula` directory.
4. Create a dedicated Ed25519 SSH key without a passphrase. Add its public key
   to `kylescudder/homebrew-tap` as a write-enabled deploy key, then store the
   private key in the Spotify TUI repository as the Actions secret
   `HOMEBREW_TAP_SSH_KEY`. Never commit or print the private key. GitHub's
   built-in token cannot write to a different repository.
5. Enable GitHub artifact attestations for the repository. The release job uses
   GitHub's OIDC token and needs no long-lived signing key.

All third-party actions are pinned to full commit SHAs. Dependabot proposes
Cargo and Actions updates weekly; review action-source changes before accepting
a new SHA.

## Rehearse a release

Run the `Release` workflow manually. An optional version input must match the
package version in `Cargo.toml`. Manual runs perform the complete build,
installer, Nix, and Homebrew test sequence, but never create a GitHub release.
They do not change the tap unless `verify_tap_write` is explicitly enabled.
That option proves the production deploy key and branch rules with one
content-neutral empty commit pushed to the tap's `main` branch; it changes tap
history but not any files.

The matrix produces:

- `spotify-tui-x86_64-unknown-linux-musl.tar.gz`
- `spotify-tui-aarch64-unknown-linux-musl.tar.gz`
- `spotify-tui-x86_64-apple-darwin.tar.gz`
- `spotify-tui-aarch64-apple-darwin.tar.gz`
- `spotify-tui-x86_64-pc-windows-msvc.zip`
- `spotify-tui-source.tar.gz`
- `spotifyd-0.4.2-source.tar.gz`
- `install.sh` and `install.ps1`
- `spotify-tui.rb`
- `SHA256SUMS`

The Spotify TUI executables in the Linux archives are musl builds to avoid
depending on a particular glibc version. Every direct archive also contains a
pinned Spotifyd runtime, its GPLv3 licence, third-party notices, README, and
example themes. Linux x86_64 uses Spotifyd's upstream default binary after
checking its hard-coded SHA-512 value. The upstream ARM64 archive requires the
obsolete OpenSSL 1.1 ABI, so ARM64 builds the same pinned source natively with
ALSA, PulseAudio, MPRIS, and Rustls enabled. That build runs inside a
digest-pinned Rust 1.88/Debian Bullseye container, giving it a glibc 2.31
baseline rather than inheriting Ubuntu 24.04's newer ABI. CI inspects both Linux
runtimes and rejects OpenSSL 1.1 or glibc requirements beyond each published
baseline before packaging. macOS and Windows apply the checked-in authenticated
local-control patch and build it with the portable Rodio backend from the exact
upstream commit; upstream does not currently publish a Windows binary. The
matching complete Spotifyd source archive and applied patches are included in
the same release and checked against a pinned SHA-256 value. Unlike the musl
Spotify TUI binary, Linux Spotifyd remains dynamically linked to standard
audio, D-Bus, and system libraries; the x86_64 build also uses OpenSSL 3.
Release smoke tests install and exercise those runtime libraries explicitly.

Release-time generation replaces `@REPOSITORY@` in the installer sources with
the actual GitHub repository. The installers preserve existing configurations;
Linux can preserve a compatible existing Spotifyd, while macOS and Windows keep
the required patched runtime upgraded. They test user-level startup integration
without requiring elevated privileges.

## Publish a release

1. Update `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` for the intended semantic
   version.
2. Complete the live product acceptance suite on every advertised platform, run
   a successful manual release rehearsal, and record that exact package version
   for `public_release_version` and every entry under `[platforms]` in
   `release-readiness.toml`. For example, a fully accepted `0.1.0` release uses
   `"0.1.0"` for all four values. Leave a value empty until that approval is
   complete. Because approvals contain the package version instead of reusable
   booleans, a later version bump automatically relocks every stale approval.
3. Run `make check`. If Nix is available, also run `nix flake check` and
   `nix build`.
4. Merge the release commit to `main` and confirm CI is green.
5. Create and push an annotated tag matching the Cargo version exactly:

   ```bash
   git tag -a v0.1.0 -m "spotify-tui 0.1.0"
   git push origin v0.1.0
   ```

The tag workflow re-runs quality checks, performs native builds on all five
targets, runs clean installer smoke tests on Linux, macOS, and Windows, builds
and tests the Homebrew formula, validates Nix, and then:

- creates GitHub provenance attestations for every release asset;
- creates the GitHub release with generated notes;
- updates `Formula/spotify-tui.rb` in `kylescudder/homebrew-tap` after the
  release assets are publicly available.

Any architecture, installer, formula, or Nix failure blocks publication. The
tap update is deliberately last so the formula can never advertise an archive
that does not exist. Release runs are serialized, and the update refuses to
downgrade an existing formula if an older tag is re-run. If the deploy-key push
fails, the GitHub release remains usable through its direct installers while
the previous Homebrew formula remains valid; fix the key or branch rule and
retry the release workflow. A tag also fails before building unless every
approval in `release-readiness.toml` matches the exact Cargo package version.

## Checksums and trust

Both installers require HTTPS and reject an artifact unless its SHA-256 matches
the release manifest. Release assembly also verifies downloaded upstream
Spotifyd binaries and source before they enter the bundle. Checksums detect
corruption and mismatched assets; they do not by themselves protect against a
compromised release account. Users who need cryptographic provenance should
download rather than pipe the installer and run `gh attestation verify` against
the repository before execution.

The scripts support local non-HTTPS paths only behind explicitly test-only
switches. CI uses those switches to test success and checksum-rejection paths
without trusting a network service.

## Homebrew maintenance

The checked-in Ruby file is deliberately a template because its repository URL,
version, and source checksum do not exist until release assembly. The generated
formula is tested from the exact staged source archive and pushed to the tap
only after every build, installer, Nix, and Homebrew check passes and the GitHub
release assets are public. The tap push is version-guarded so re-running an old
tag cannot replace a newer formula.

Do not merge or advertise the macOS formula as functionally complete until its
local-control playback, service, and audio paths pass the product acceptance
suite. The same rule applies to the Windows installer.

The clean-install rehearsal must also prove zero-command daemon startup: the
Linux installer uses `systemctl --user enable --now`, the macOS installer
bootstraps its LaunchAgent, the Homebrew formula service is registered by the
first TUI launch, and the Windows installer creates its Startup entry and starts
Spotifyd in the current session. No advertised path may require a user to run
`spotifyd --no-daemon` manually.
