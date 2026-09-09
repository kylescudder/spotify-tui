# Spotify TUI handover

## Objective

Build a polished, keyboard-first Spotify TUI that remains useful when Spotify's
Web API is unavailable, rate-limited, or changes. Audio playback is always local
through `spotifyd`; optional Spotify Web API catalogue access supplies search
and browse data and starts exact catalogue selections on the active Spotifyd
device without becoming a requirement for the now-playing controller.

The target machine is Kyle's NixOS workstation (`stevie`) using Hyprland,
Ghostty, PipeWire, Home Manager, and the dotfiles repository at
`~/Documents/Repos/dotfiles`.

Version 1 has three distribution routes:

- A Nix flake for Linux on x86_64 and aarch64.
- Homebrew for macOS on Apple Silicon and Intel.
- Direct release installers: `curl | sh` for Linux/macOS and a PowerShell
  installer for Windows. Windows x86_64 is the initial Windows target; add other
  architectures only after CI and live validation exist for them.

Linux runtime acceptance is complete. The macOS and Windows playback adapter
and packaging are implemented, but both still require clean-machine live
acceptance before their installers can be released; installing successfully
without working controls is not considered platform support.

## Product boundary

Version 1 must provide:

- Current track, artist, album, playback state, elapsed time, and duration.
- Play/pause, previous, next, seek, and volume controls.
- Album artwork rendered with the best supported terminal graphics protocol,
  preferring Kitty graphics in Ghostty and degrading to a text/block fallback.
- Keyboard-first navigation, including Vim-style bindings.
- User customization through a TOML config file, including configurable colour
  schemes, with polished defaults when no config is present.
- First-run playback onboarding delegated to `spotifyd authenticate`; the TUI
  must never collect a password or inspect/store Spotifyd's playback
  credential.
- Zero-command Spotifyd lifecycle: distribution routes install or expose the
  daemon, persistent install routes add user startup integration, and every TUI
  launch ensures Spotifyd is running without asking the user to start it.
- Phone-free Linux session bootstrap: activate an authenticated Spotifyd over
  its local D-Bus control interface, preserving a resumable context or opening
  an optional configured startup Spotify URI.
- Optional catalogue search across artists, albums, tracks, and playlists;
  artist pages listing releases; album pages listing tracks; and local playback
  of a selected track or playlist through its Spotify URI.
- Lazy in-TUI catalogue onboarding through Authorization Code with PKCE, using
  a user-supplied Spotify developer client ID and a loopback redirect. The first
  search must open the browser and resume automatically without asking the user
  to leave the TUI or run another command. Never ask for or store a client
  secret or Spotify password.
- Clear empty, disconnected, paused, loading, and error states.
- A visually intentional layout that works at common terminal sizes.

Version 1 does not include saved-library browsing, playlist editing, lyrics, or
recommendations. Catalogue browsing is optional and must never be required for
the controller to start or work.

## Platform and reliability seams

Keep `PlaybackSource`, `ArtworkSource`, `CatalogSource`, and service lifecycle
control platform-neutral. Platform adapters must produce the same normalized
events and commands so `AppState` and the view contain no operating-system
branches.

`SpotifydLifecycle` is the service-management seam. Its small interface ensures
the daemon is running or restarts it after authentication. Linux prefers the
installed systemd user unit, falls back to a transient user unit for raw Nix
and development launches, and finally uses a detached process where systemd is
unavailable. macOS prefers launchd, registers the Homebrew formula service when
needed, and has the same detached fallback. Windows uses its installed Startup
entry and a detached process fallback. Lifecycle failures are recoverable from
the TUI's retry action and never instruct the user to run Spotifyd manually.

On Linux, use the session MPRIS interface exposed by `spotifyd` as the source of
truth for playback state and controls. The existing config already sets:

```toml
use_mpris = true
dbus_type = "session"
```

Spotifyd 0.3.4 and newer use a process-unique bus name of the form
`org.mpris.MediaPlayer2.spotifyd.instance<PID>`; older versions used
`org.mpris.MediaPlayer2.spotifyd`. Discover the currently owned matching name
rather than assuming either literal value.

Read properties and subscribe to D-Bus change signals instead of polling on a
short timer. Interpolate progress locally between authoritative position updates.
Spotifyd may transiently reject `Position` while an empty session activates;
treat that single property as zero without discarding valid playback status,
metadata, or volume.
Treat reconnecting to MPRIS as normal runtime behavior: `spotifyd` may start,
stop, or temporarily disappear while the TUI remains open.

Spotifyd exposes `rs.spotifyd.Controls.TransferPlayback` before its MPRIS player
exists. Use that local interface to make Spotifyd active, then rediscover the
process-unique MPRIS name. Do not require the user to select the device from a
phone or another Spotify client. If `playback.startup_uri` is configured, open
it through MPRIS after activation; otherwise preserve Spotify's existing
resumable context.

Spotifyd does not expose the Linux D-Bus/MPRIS interface on macOS or Windows.
Those platforms therefore use the `local_control` feature patched into the
pinned bundled Spotifyd 0.4.2 source. The daemon listens on an ephemeral
loopback port, writes its current address and a random authentication token to
the platform's per-user local-data directory, and rejects unauthenticated or
non-loopback requests. `LocalControlPlaybackSource` discovers that endpoint on
each request, so a daemon restart can change ports without restarting the TUI.
It exposes the same normalized metadata, progress, artwork URL, activation,
play/pause, previous/next, seek, volume, and exact-URI commands as Linux MPRIS.
This transport never uses the Spotify Web API. The source patch and application
script live in `packaging/spotifyd`; release builds and the Homebrew formula
apply it to the pinned upstream commit before compiling the Rodio backend.
Environment overrides exist for controlled tests, but normal users do not
configure the endpoint or token.

On Linux, album art comes from the MPRIS `mpris:artUrl` value. `ArtworkSource`
enforces HTTPS, download timeouts, transfer and decode limits, downsizes large
images, and keeps a bounded in-memory LRU cache. Catalogue navigation
optimistically prefetches up to four nearby images on a dedicated worker;
the selected image has its own foreground worker and therefore never waits for
speculative work. Cache hits bypass that worker and reach `AppState` before the
next frame, while pending fetch and terminal-encoding results use a 16 ms poll
interval rather than the 250 ms idle interval. Artist pages preview the selected
release artwork and fall back to the artist image when needed. Download/decode
and terminal resize/encoding run on separate workers. `AppState` rejects results
whose track revision is stale. Ghostty uses the detected Kitty protocol;
unsupported terminals use Unicode half blocks, and missing or invalid art
renders a text placeholder. The macOS and Windows playback adapters must supply
equivalent artwork metadata or a documented local alternative.

## Recommended implementation

- Language: Rust.
- TUI: `ratatui` + `crossterm`.
- Linux D-Bus/MPRIS: `zbus` with typed internal adapters.
- Async runtime: `tokio`.
- Images: `ratatui-image`, preferring Kitty graphics with a block-character
  fallback.
- HTTP and image decode: a small client with timeouts plus the `image` crate.
- Errors: typed domain errors internally; concise status messages in the UI.
- Tests: state reducer and formatting tests without a live Spotify account.

Keep these seams explicit:

```text
platform player adapter -> PlaybackSource -> AppState -> Ratatui view
art URL/cache           -> ArtworkSource -------^
keyboard                -> Command dispatcher -> PlaybackSource
Spotify Web API         -> CatalogSource -> BrowserState -> Ratatui view
selected Spotify URI    ----------------> PlaybackSource
```

The view must depend on `AppState`, not D-Bus, HTTP, or subprocess handles. This
is the main test seam.

## Spotify API constraint

Catalogue search uses a user-supplied Spotify developer client ID and
Authorization Code with PKCE. The refresh token is stored in the user's state
directory with owner-only Unix permissions; no client secret is accepted.
`CatalogSource` normalizes Search, artist releases, and album tracks while the
worker owns HTTP and token refresh away from the render/input thread. Selecting
playable content passes its URI to `PlaybackSource`, so the Web API never
streams audio or becomes the playback transport.
Source initialization is lazy: a first search with no usable token opens the
browser from the catalogue worker, waits for the loopback callback, and then
continues that same request. Authentication errors remain retryable without
restarting the TUI.

Spotify's Development Mode restrictions still apply. Spotify uses
per-developer quota buckets, returns `429` for quota/rate limiting, limits apps
to five allowlisted users, and changed or removed endpoints in 2026. Catalogue
features must remain lazy, optional, and resilient to `403`/`429` responses.

Primary references:

- https://developer.spotify.com/documentation/web-api/concepts/quota-modes
- https://developer.spotify.com/documentation/web-api/concepts/rate-limits
- https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow
- https://developer.spotify.com/documentation/web-api/references/changes/february-2026

## Relevant existing files

- `~/Documents/Repos/dotfiles/spotifyd/spotifyd.conf`
- `~/Documents/Repos/dotfiles/scripts/songchange`
- `~/Documents/Repos/dotfiles/home/modules/media.nix`
- `~/Documents/Repos/dotfiles/home/modules/dotfiles.nix`
- `~/Documents/Repos/dotfiles/hyprland/hyprland.lua`

The dotfiles currently install `spotify-player` and launch `spotify_player` in a
Ghostty window on workspace 10. Preserve that setup until this replacement has
passed its acceptance checks. Integration into NixOS and Hyprland is a final
handoff step, not part of initial scaffolding.

## Distribution and CI implementation

The release infrastructure is implemented in this repository:

- `.github/workflows/ci.yml` runs locked formatting, Clippy, and tests on Linux,
  macOS, and Windows; validates the Nix flake; and tests both installer families.
  Actions are pinned to immutable commits and untrusted jobs have read-only
  permissions.
- `flake.nix` and `flake.lock` expose the Linux package, app, checks,
  development shell, and `nix/home-manager-module.nix`. The package wraps
  `spotifyd` and systemd tools onto `PATH`, pins the daemon executable for the
  lifecycle adapter, and leaves an existing Spotifyd config unchanged. Raw Nix
  launches get an on-demand transient user unit; Home Manager provides the
  persistent login service.
- `.github/workflows/release.yml` accepts manual non-publishing rehearsals and
  semantic tags. It natively builds Linux x86_64/aarch64, macOS Intel/Apple
  Silicon, and Windows x86_64 artifacts, generates checksums, creates GitHub
  provenance attestations, publishes only after every packaging job passes,
  then updates the Homebrew tap after the assets are public. Manual rehearsals
  can explicitly validate deploy-key and branch-policy write access with one
  content-neutral empty commit to the tap's `main` branch.
  `release-readiness.toml` additionally
  blocks public tags until the exact Cargo package version is explicitly
  approved for public release and live runtime acceptance on all three
  platforms; an old version's approvals cannot carry across a version bump.
- `scripts/install.sh` provides the HTTPS-only `curl | sh` path for Linux/macOS;
  `scripts/install.ps1` provides the PowerShell path for Windows. Both select the
  matching release, support pinned versions and user-writable destinations,
  verify SHA-256 before replacing binaries, preserve existing configuration,
  install a pinned bundled Spotifyd when necessary on Linux and always install
  the compatible patched runtime on macOS/Windows, create
  platform user-startup definitions, and start them in the installation
  session. Install, upgrade, dependency opt-out, service, invalid-version, and
  tampered-artifact paths have offline tests.
- Direct archives include Spotifyd's GPLv3 licence and publish its complete
  corresponding 0.4.2 source plus the applied patch beside the binaries. Linux
  consumes hash-pinned upstream MPRIS builds; macOS and Windows build the Rodio
  backend plus local control from pinned upstream commit
  `c5b94367014856a8c541dea565cbd332e034fb9e`.
- `packaging/homebrew/spotify-tui.rb.template` is rendered with the tagged source
  checksum and defines the Spotifyd service used by the lifecycle adapter. It is
  styled, audited, installed, and tested on macOS. A successful tag pushes the
  tested formula to `kylescudder/homebrew-tap` using its scoped SSH deploy key
  after the GitHub release assets become public. Release runs are serialized,
  and a formula version guard prevents an older tag rerun from downgrading the
  tap.
- `LICENSE`, `CHANGELOG.md`, the README installation/verification instructions,
  and `docs/releasing.md` complete the operator-facing release surface.

The canonical product repository is `kylescudder/spotify-tui`, and releases
publish formula updates through the existing `kylescudder/homebrew-tap`
repository (the `kylescudder/tap` Homebrew tap). The remaining infrastructure
activation is to enable the required repository protections and attestations
and run the release workflow on GitHub. The scoped cross-repository deploy key
and `HOMEBREW_TAP_SSH_KEY` secret are configured; their production-branch write
still needs the explicit rehearsal described below. The workflow stamps the
actual product repository into release installers at build time, so forks also
remain functional.

The infrastructure can build macOS and Windows packages, but those packages must
not be advertised as functionally complete until the platform runtime adapters
below pass live acceptance.

## Remaining execution sequence

The Linux MPRIS vertical slice is implemented: the diagnostic has been validated
against Spotifyd 0.4.2, the TUI subscribes to property and seek signals, commands
flow through `PlaybackSource`, and the supervisor automatically activates an
inactive Spotifyd and reconnects with bounded backoff. An optional
`playback.startup_uri` guarantees a playable context without a phone.
Deterministic fake-source tests cover updates, commands, failures, activation,
startup URI loading, manual retry, and daemon recovery; a private-D-Bus test
covers activation and rediscovery across process-unique names. The artwork
vertical slice is also implemented behind `ArtworkSource`, including bounded
fetch/decode, eight-entry in-memory LRU caching, non-blocking nearby-image
prefetch, same-frame cache hits, responsive pending-result polling,
stale-result rejection, Kitty rendering, a half-block fallback, selected-release
previews on artist pages, responsive now-playing and catalogue artwork, and
normal/narrow layout tests. The catalogue vertical
slice is implemented behind `CatalogSource`: PKCE authentication and token
refresh, typed search results, artist releases, album tracks, keyboard history,
stale-result rejection, and exact URI playback on the active Spotifyd device all
have deterministic tests. Catalogue playback deliberately bypasses Spotifyd
0.4.2's off-by-one MPRIS `OpenUri` implementation, while a Web API `404` falls
back to the selected local URI instead of becoming a fatal player state. The
macOS/Windows local-control adapter and patched Spotifyd packaging are also
implemented, and controlled tests cover actionable catalogue `403`/`429`
mapping. The remaining work is:

Live validation on `stevie` has confirmed Spotifyd OAuth, phone-free activation,
automatic recovery after restarting Spotifyd, a successful `nix run .` build,
in-TUI catalogue authorization and search, artist/release navigation, responsive
catalogue imagery, selected-release artwork, instant revisiting of cached or
prefetched images, exact selected-track playback, cached authorization after a
restart, and responsive layouts. Keep those paths in regression coverage, but
they are no longer open implementation tasks.

1. Live-validate the macOS and Windows platform adapters.
   - Live-test the Homebrew/launchd and Windows Startup lifecycle adapters
     alongside the implemented local-control playback adapter.
   - Verify both Apple Silicon and Intel builds in CI; functional validation on
     real hardware is required for every architecture advertised by the formula.
   - Verify the Windows x86_64 build in CI and on a clean Windows machine before
     publishing its installer.

2. Harden the complete runtime and perform remaining live acceptance on macOS
   and Windows.
   - Exercise a fresh Spotifyd OAuth approval, cancellation, service restart,
     network loss, pause/resume, daemon loss, and daemon reconnection.
   - Test repeated track changes, missing art, small terminals, shutdown during
     background work, and terminal restoration after failures.
   - Repeat equivalent playback, authentication, service lifecycle, and failure
     tests on a clean macOS Homebrew installation.
   - Repeat them on a clean Windows installation produced by the PowerShell
     installer.
   - On each direct-install platform, prove the bundled Spotifyd starts from its
     generated user startup definition, preserves an existing config, and can be
     omitted explicitly without affecting the Spotify TUI installation.

3. Activate and prove the release infrastructure.
   - Run the non-publishing GitHub Actions rehearsal and require every Linux,
     macOS, Windows, Nix, installer, and Homebrew job to pass.
   - Explicitly enable `verify_tap_write` during one manual rehearsal to validate
     `HOMEBREW_TAP_SSH_KEY` against the tap's production `main` branch. This adds
     an empty commit but does not change tap files.
   - Prove that a tagged release publishes and attests its archives before it
     updates `Formula/spotify-tui.rb` in `kylescudder/homebrew-tap`.
   - Do not create a public product tag until platform runtime acceptance passes.

4. Cut over the workstation only after acceptance.
   - Update the dotfiles/Home Manager package and Hyprland workspace-10 launch
     command, perform a clean NixOS rebuild, and retain a simple rollback to
     `spotify_player` until the new setup has been used successfully.

## Remaining acceptance checks

Version 1 is complete when all of the following are reproducible after the
remaining implementation:

- Play/pause, previous, next, seek, and volume work through the platform's local
  playback adapter without a Spotify Web API request.
- Cancelling `spotify-tui auth` is safe and leaves any existing Spotifyd
  credential usable.
- Metadata, interpolated progress, and artwork stay correct across ten
  consecutive track changes, including pause/resume and seeks.
- Network loss and restarting Spotifyd do not crash, freeze, or leave stale
  state or artwork onscreen.
- Missing or invalid artwork degrades cleanly.
- An authenticated catalogue user can search artists, albums, tracks, and
  playlists; traverse artist → release → track; start the exact selected URI on
  the active Spotifyd device; see contextual catalogue artwork; and return
  through browser history without blocking playback.
- Catalogue auth cancellation, token refresh, `403`, `429`, network failure,
  and a missing or corrupt cache produce actionable errors and never prevent the
  local controller from launching.
- The complete UI is usable in Ghostty at 80x24 and the normal workspace size,
  in a supported macOS terminal, in Windows Terminal, and in the documented
  narrow fallback.
- Pull-request CI passes Rust and Nix checks on Linux plus Rust and build checks
  on macOS and Windows from a clean checkout.
- `nix build`, `nix flake check`, and the Home Manager integration succeed on a
  clean NixOS system, and the workspace-10 launcher survives a clean rebuild.
- A release rehearsal proves the Nix flake on supported Linux systems and the
  Homebrew formula on supported macOS systems; the formula passes style, audit,
  install, and smoke tests against the tagged source checksum.
- The POSIX and PowerShell installers select the correct release artifact,
  reject checksum mismatches, install Spotifyd plus its licence without
  elevation by default, preserve existing configuration, keep the required
  macOS/Windows runtime compatible, and pass clean
  install/upgrade/version/service smoke tests on every advertised platform.
- Formatting, Clippy, all unit/integration tests, and all packaging checks pass at
  the release commit.

## Recorded Linux live validation

These commands were used on `stevie` inside the graphical session with a track
loaded in Spotifyd:

```bash
playerctl -p spotifyd status
playerctl -p spotifyd metadata
busctl --user list | grep 'org.mpris.MediaPlayer2.spotifyd'
cargo run --bin spotify-tui-diagnose
cargo run --bin spotify-tui
```

The corresponding TUI checks covered Space, `p`/`n`, `h`/`l`, `j`/`k`, daemon
restart recovery, track and artwork changes, catalogue navigation, and
responsive layouts. Keep these commands as a reproducible regression recipe
before the workstation cutover.

Equivalent end-to-end validation from a fresh Homebrew install is required on a
macOS test machine. The Homebrew formula is not release-ready until that
validation is documented and repeatable.

Equivalent end-to-end validation from a fresh PowerShell-script installation is
required on a clean Windows machine. The Windows installer is not release-ready
until that validation is documented and repeatable.
