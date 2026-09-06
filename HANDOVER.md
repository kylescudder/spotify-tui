# Spotify TUI handover

## Objective

Build a polished Spotify now-playing TUI that remains useful when Spotify's Web
API is rate-limited or changes. The first release is a local controller and
visualizer for `spotifyd`; it does not browse Spotify through the Web API.

The target machine is Kyle's NixOS workstation (`stevie`) using Hyprland,
Ghostty, PipeWire, Home Manager, and the dotfiles repository at
`~/Documents/Repos/dotfiles`.

Version 1 has three distribution routes:

- A Nix flake for Linux on x86_64 and aarch64.
- Homebrew for macOS on Apple Silicon and Intel.
- Direct release installers: `curl | sh` for Linux/macOS and a PowerShell
  installer for Windows. Windows x86_64 is the initial Windows target; add other
  architectures only after CI and live validation exist for them.

The current runtime is Linux-only. Compatible playback, service, and
audio-capture implementations are therefore required before the macOS or
Windows installers can be released; installing successfully without working
controls is not considered platform support.

## Product boundary

Version 1 must provide:

- Current track, artist, album, playback state, elapsed time, and duration.
- Play/pause, previous, next, seek, and volume controls.
- Album artwork rendered with the best supported terminal graphics protocol,
  preferring Kitty graphics in Ghostty and degrading to a text/block fallback.
- A responsive audio spectrum sourced from the local audio output.
- Keyboard-first navigation, including Vim-style bindings.
- User customization through a TOML config file, including configurable colour
  schemes, with polished defaults when no config is present.
- First-run account onboarding delegated to `spotifyd authenticate`; the TUI
  must never collect, inspect, or store Spotify credentials itself.
- Clear empty, disconnected, paused, loading, and error states.
- A visually intentional layout that works at common terminal sizes.

Version 1 does not include search, library browsing, playlist editing, lyrics,
recommendations, or direct Spotify Web API authentication. These are later,
optional modules and must not be required for the controller to start or work.

## Platform and reliability seams

Keep `PlaybackSource`, `ArtworkSource`, `SpectrumSource`, and service lifecycle
control platform-neutral. Platform adapters must produce the same normalized
events and commands so `AppState` and the view contain no operating-system
branches.

On Linux, use the session MPRIS interface exposed by `spotifyd` as the source of
truth for playback state and controls. The existing config already sets:

```toml
use_mpris = true
dbus_type = "session"
```

Expected bus name: `org.mpris.MediaPlayer2.spotifyd`.

Read properties and subscribe to D-Bus change signals instead of polling on a
short timer. Interpolate progress locally between authoritative position updates.
Treat reconnecting to MPRIS as normal runtime behavior: `spotifyd` may start,
stop, or temporarily disappear while the TUI remains open.

Spotifyd does not expose the Linux D-Bus/MPRIS interface on macOS or Windows.
Before publishing installers for either platform, prototype and select
maintainable local transports that provide equivalent metadata, progress,
controls, volume, and reconnect behavior without making Spotify Web API access
mandatory. Record those decisions before implementation; do not silently ship
reduced, display-only builds.

On Linux, album art should come from the MPRIS `mpris:artUrl` value. The macOS
and Windows playback adapters must supply equivalent artwork metadata or a
documented local alternative. Cache downloaded art by URL or track ID, bound
the cache size, and render a text placeholder whenever art is missing or invalid.

The audio spectrum cannot come from MPRIS. On Linux, use `cava` configured with
PipeWire/PulseAudio capture and a machine-readable raw output consumed by the
TUI. Prove equivalent macOS/CoreAudio and Windows/WASAPI capture paths before
their installers are released. Keep every implementation behind a small
`SpectrumSource` interface so it can be replaced independently later.

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

Keep these boundaries explicit:

```text
platform player adapter -> PlaybackSource -> AppState -> Ratatui view
platform audio capture  -> SpectrumSource -----^
art URL/cache           -> ArtworkSource -------^
keyboard                -> Command dispatcher -> PlaybackSource
```

The view must depend on `AppState`, not D-Bus, HTTP, or subprocess handles. This
is the main test seam.

## Spotify API constraint

Creating another conventional Web API client would inherit Spotify's current
Development Mode restrictions. Spotify applies per-developer quota buckets,
returns `429` for quota/rate limiting, limits Development Mode apps to allowlisted
users, and changed or removed endpoints in 2026. If Web API features are added,
they must be lazy, cached, optional, and resilient to `403`/`429` responses.

Primary references:

- https://developer.spotify.com/documentation/web-api/concepts/quota-modes
- https://developer.spotify.com/documentation/web-api/concepts/rate-limits
- https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide

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
  `spotifyd` and `cava` onto `PATH` without changing an existing Spotifyd config.
- `.github/workflows/release.yml` accepts manual non-publishing rehearsals and
  semantic tags. It natively builds Linux x86_64/aarch64, macOS Intel/Apple
  Silicon, and Windows x86_64 artifacts, generates checksums, creates GitHub
  provenance attestations, and publishes only after every packaging job passes.
  `release-readiness.toml` additionally blocks public tags until live runtime
  acceptance is explicitly recorded for all three platforms.
- `scripts/install.sh` provides the HTTPS-only `curl | sh` path for Linux/macOS;
  `scripts/install.ps1` provides the PowerShell path for Windows. Both select the
  matching release, support pinned versions and user-writable destinations,
  verify SHA-256 before replacing binaries, preserve existing dependencies and
  configuration, install a pinned bundled Spotifyd when necessary, and create
  platform user-startup definitions. Install, upgrade, dependency opt-out,
  service, invalid-version, and tampered-artifact paths have offline tests.
- Direct archives include Spotifyd's GPLv3 licence and publish its complete
  corresponding 0.4.2 source beside the binaries. Linux consumes hash-pinned
  upstream MPRIS builds; macOS and Windows build the portable Rodio backend from
  pinned upstream commit `c5b94367014856a8c541dea565cbd332e034fb9e`.
- `packaging/homebrew/spotify-tui.rb.template` is rendered with the tagged source
  checksum, styled, audited, installed, and tested on macOS. A successful tag
  opens a reviewable formula pull request in `kylescudder/homebrew-tap`; it never
  pushes an unreviewed checksum to the tap's default branch.
- `LICENSE`, `CHANGELOG.md`, the README installation/verification instructions,
  and `docs/releasing.md` complete the operator-facing release surface.

This checkout still has no GitHub remote. The intended canonical product
repository is `kylescudder/spotify-tui`, and releases publish formula updates
through the existing `kylescudder/homebrew-tap` repository (the
`kylescudder/tap` Homebrew tap). The remaining infrastructure activation is to
create the product repository, enable branch protection and attestations,
configure the cross-repository tap token described in `docs/releasing.md`, and
run the workflows on GitHub. The workflow stamps the actual product repository
into release installers at build time, so forks also remain functional.

The infrastructure can build macOS and Windows packages, but those packages must
not be advertised as functionally complete until the platform runtime adapters
below pass live acceptance.

## Remaining execution sequence

1. Finish the Linux MPRIS runtime and command seam.
   - Validate the existing diagnostic against a playing Spotifyd instance on the
     graphical session bus.
   - Add a long-lived async supervisor that subscribes to MPRIS property and seek
     signals, dispatches normalized events to `AppState`, and reconnects with
     bounded backoff when Spotifyd disappears.
   - Extend `PlaybackSource` with play/pause, previous, next, relative seek, and
     volume operations. Cover command mapping, source failures, stale events,
     and reconnection with fakes so development does not require Spotify.

2. Prove and implement the macOS and Windows platform adapters.
   - Prototype non-Web-API playback/control transports against Spotifyd on each
     platform, record the selected designs, and implement them behind
     `PlaybackSource`.
   - Refactor post-authentication service restart behind a platform seam; keep
     the current systemd user-service controller on Linux and add a tested
     `brew services`/launchd controller on macOS plus an appropriate Windows
     process/service controller.
   - Prove local CoreAudio and WASAPI spectrum capture paths and expose them
     through `SpectrumSource`.
   - Verify both Apple Silicon and Intel builds in CI; functional validation on
     real hardware is required for every architecture advertised by the formula.
   - Verify the Windows x86_64 build in CI and on a clean Windows machine before
     publishing its installer.

3. Build the complete responsive now-playing UI and keyboard dispatcher.
   - Render track, artist, album, playback status, interpolated progress,
     duration, volume, help, and all loading/empty/disconnected/error states.
   - Provide discoverable arrow-key and Vim-style bindings for every v1 control.
   - Use a deliberate normal layout and a usable narrow fallback rather than
     allowing widgets to truncate unpredictably.

4. Add artwork behind an `ArtworkSource` boundary.
   - Fetch `mpris:artUrl` with strict timeouts and size limits, decode it off the
     render path, and use a bounded cache keyed by URL or track identity.
   - Prefer Kitty graphics in Ghostty, provide a block-character/text fallback,
     and reject stale results when the track revision changes.

5. Add spectrum visualization behind a `SpectrumSource` boundary.
   - Launch and supervise the selected platform capture process with a
     machine-readable raw output format: PipeWire/PulseAudio on Linux and the
     proven CoreAudio path on macOS or WASAPI path on Windows.
   - Bound and validate samples so missing, stopped, slow, or malformed `cava`
     output never blocks input or rendering.

6. Harden the complete runtime and perform live acceptance on Linux, macOS, and
   Windows.
   - Exercise a fresh Spotifyd OAuth approval, cancellation, service restart,
     network loss, pause/resume, daemon loss, and daemon reconnection.
   - Test repeated track changes, missing art, missing/stopped spectrum capture,
     small terminals, shutdown during background work, and terminal restoration
     after failures.
   - Repeat equivalent playback, authentication, audio, and failure tests on a
     clean macOS Homebrew installation.
   - Repeat them on a clean Windows installation produced by the PowerShell
     installer.
   - On each direct-install platform, prove the bundled Spotifyd starts from its
     generated user startup definition, preserves an existing config, and can be
     omitted explicitly without affecting the Spotify TUI installation.

7. Activate and prove the release infrastructure after the external repositories
   exist.
   - Run the non-publishing GitHub Actions rehearsal and require every Linux,
     macOS, Windows, Nix, installer, and Homebrew job to pass.
   - Configure the `HOMEBREW_TAP_TOKEN` integration and prove that a tagged
     release opens a reviewable formula pull request in
     `kylescudder/homebrew-tap` without direct writes to its default branch.
   - Do not create a public product tag until platform runtime acceptance passes.

8. Cut over the workstation only after acceptance.
   - Update the dotfiles/Home Manager package and Hyprland workspace-10 launch
     command, perform a clean NixOS rebuild, and retain a simple rollback to
     `spotify_player` until the new setup has been used successfully.

## Remaining acceptance checks

Version 1 is complete when all of the following are reproducible after the
remaining implementation:

- Starting before Spotifyd shows the correct state and reconnects later without
  restarting the TUI or busy-polling the platform playback transport.
- Play/pause, previous, next, seek, and volume work through the platform's local
  playback adapter without a Spotify Web API request.
- A fresh `spotify-tui auth` browser flow succeeds, cancellation is safe, and the
  credential remains exclusively owned by Spotifyd.
- Metadata, interpolated progress, artwork, and spectrum stay correct across ten
  consecutive track changes, including pause/resume and seeks.
- Network loss and restarting Spotifyd do not crash, freeze, or leave stale
  state or artwork onscreen.
- Missing/invalid artwork and missing/stopped/malformed spectrum capture degrade
  cleanly.
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
  elevation by default, preserve existing dependency state, and pass clean
  install/upgrade/version/service smoke tests on every advertised platform.
- Formatting, Clippy, all unit/integration tests, and all packaging checks pass at
  the release commit.

## Remaining Linux live validation commands

Run these on `stevie` inside the graphical session once Spotifyd is available:

```bash
playerctl -p spotifyd status
playerctl -p spotifyd metadata
busctl --user introspect \
  org.mpris.MediaPlayer2.spotifyd \
  /org/mpris/MediaPlayer2
cargo run --bin spotify-tui-diagnose
```

These commands should see the same player and normalized track state before the
workstation cutover begins.

Equivalent end-to-end validation from a fresh Homebrew install is required on a
macOS test machine once the macOS playback and audio transports have been
selected. The Homebrew formula is not release-ready until that validation is
documented and repeatable.

Equivalent end-to-end validation from a fresh PowerShell-script installation is
required on a clean Windows machine once the Windows playback and WASAPI
transports have been selected. The Windows installer is not release-ready until
that validation is documented and repeatable.
