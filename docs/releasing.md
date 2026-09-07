# Release operations

The repository treats packaging as a tested product surface. Pull requests run
Rust checks on Linux, macOS, and Windows, validate the locked Nix flake, and test
the POSIX and PowerShell installers. A semantic version tag assembles and tests
every release path before publishing anything.

## One-time repository setup

1. Create `kylescudder/spotify-tui` and add it as `origin`. This checkout
   currently has no remote, so the documented public installer URL is not live
   yet. If a different canonical repository is chosen, update the Cargo, Nix,
   and README metadata; generated installers use the actual Actions repository
   automatically.
2. Protect `main` and require the CI jobs before merging. Do not use
   `pull_request_target`; untrusted pull requests receive read-only repository
   permissions and no release secrets.
3. The release workflow targets the existing `kylescudder/homebrew-tap`
   repository. Its Homebrew name is `kylescudder/tap`, and release formulae are
   placed in its existing `Formula` directory.
4. Create a fine-grained token with contents and pull-request write access only
   to `kylescudder/homebrew-tap`, then store it in the Spotify TUI repository as
   the Actions secret `HOMEBREW_TAP_TOKEN`. GitHub's built-in token cannot write
   to a different repository.
5. Enable GitHub artifact attestations for the repository. The release job uses
   GitHub's OIDC token and needs no long-lived signing key.

All third-party actions are pinned to full commit SHAs. Dependabot proposes
Cargo and Actions updates weekly; review action-source changes before accepting
a new SHA.

## Rehearse a release

Run the `Release` workflow manually. An optional version input must match the
package version in `Cargo.toml`. Manual runs perform the complete build,
installer, Nix, and Homebrew test sequence but never create a GitHub release or
tap pull request.

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
example themes. Linux uses Spotifyd's upstream default binaries after checking
hard-coded SHA-512 values. macOS and Windows build the portable Rodio backend
from the exact upstream commit; upstream does not currently publish a Windows
binary. The matching complete Spotifyd source archive is included in the same
release and checked against a pinned SHA-256 value. Unlike the musl Spotify TUI
binary, upstream's Linux Spotifyd binary remains dynamically linked to standard
audio, D-Bus, OpenSSL, and system libraries; release smoke tests install and
exercise those runtime libraries explicitly.

Release-time generation replaces `@REPOSITORY@` in the installer sources with
the actual GitHub repository. The installers preserve existing Spotifyd
installations and configurations, install the bundled runtime when necessary,
and test user-level startup integration without requiring elevated privileges.

## Publish a release

1. Update `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` for the intended semantic
   version.
2. Complete the live product acceptance suite on every advertised platform, run
   a successful manual release rehearsal, and set each platform plus
   `public_release` to `true` in `release-readiness.toml`. This reviewed file is
   the mechanical guard against publishing installable but non-functional
   packages.
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
- opens a formula-update pull request against `kylescudder/homebrew-tap`.

Any architecture, installer, formula, or Nix failure blocks publication. The
workflow never publishes a partial release. A tag also fails before building if
`release-readiness.toml` has not been explicitly unlocked.

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
formula is tested from the exact staged source archive, uploaded with the GitHub
release, and copied into the tap on a review branch. No workflow pushes an
unreviewed checksum directly to the tap's default branch.

Do not merge or advertise the macOS formula as functionally complete until the
macOS playback, service, and audio adapters pass the product acceptance suite.
The same rule applies to the Windows installer and its native adapters.

The clean-install rehearsal must also prove zero-command daemon startup: the
Linux installer uses `systemctl --user enable --now`, the macOS installer
bootstraps its LaunchAgent, the Homebrew formula service is registered by the
first TUI launch, and the Windows installer creates its Startup entry and starts
Spotifyd in the current session. No advertised path may require a user to run
`spotifyd --no-daemon` manually.
