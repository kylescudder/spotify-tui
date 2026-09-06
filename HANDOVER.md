# Spotify TUI handover

## Objective

Build a polished Linux Spotify now-playing TUI that remains useful when Spotify's
Web API is rate-limited or changes. The first release is a local controller and
visualizer for `spotifyd`; it does not browse Spotify through the Web API.

The target machine is Kyle's NixOS workstation (`stevie`) using Hyprland,
Ghostty, PipeWire, Home Manager, and the dotfiles repository at
`~/Documents/Repos/dotfiles`.

## Product boundary

Version 1 must provide:

- Current track, artist, album, playback state, elapsed time, and duration.
- Play/pause, previous, next, seek, and volume controls.
- Album artwork rendered in Ghostty through the Kitty graphics protocol.
- A responsive audio spectrum sourced from the local PipeWire output.
- Keyboard-first navigation, including Vim-style bindings.
- Clear empty, disconnected, paused, loading, and error states.
- A visually intentional layout that works at common terminal sizes.

Version 1 does not include search, library browsing, playlist editing, lyrics,
recommendations, or direct Spotify Web API authentication. These are later,
optional modules and must not be required for the controller to start or work.

## Reliability seam

Use the session MPRIS interface exposed by `spotifyd` as the source of truth for
playback state and controls. The existing config already sets:

```toml
use_mpris = true
dbus_type = "session"
```

Expected bus name: `org.mpris.MediaPlayer2.spotifyd`.

Read properties and subscribe to D-Bus change signals instead of polling on a
short timer. Interpolate progress locally between authoritative position updates.
Treat reconnecting to MPRIS as normal runtime behavior: `spotifyd` may start,
stop, or temporarily disappear while the TUI remains open.

Album art should come from the MPRIS `mpris:artUrl` value. Cache downloaded art
by URL or track ID, bound the cache size, and render a text placeholder whenever
art is missing or invalid.

The audio spectrum cannot come from MPRIS. For the initial implementation, use
`cava` configured with PipeWire/PulseAudio capture and a machine-readable raw
output consumed by the TUI. Keep this behind a small `SpectrumSource` interface
so a native PipeWire implementation can replace it later.

## Recommended implementation

- Language: Rust.
- TUI: `ratatui` + `crossterm`.
- D-Bus/MPRIS: `zbus` with typed internal adapters.
- Async runtime: `tokio`.
- Images: `ratatui-image`, preferring Kitty graphics with a block-character
  fallback.
- HTTP and image decode: a small client with timeouts plus the `image` crate.
- Errors: typed domain errors internally; concise status messages in the UI.
- Tests: state reducer and formatting tests without a live Spotify account.

Keep these boundaries explicit:

```text
spotifyd/MPRIS -> PlaybackSource -> AppState -> Ratatui view
cava/PipeWire  -> SpectrumSource -----^
art URL/cache  -> ArtworkSource -------^
keyboard       -> Command dispatcher -> PlaybackSource
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

## Execution sequence

1. Initialize this directory as a Git repository and create a minimal Cargo app.
   Add formatting, Clippy, and unit-test commands. Completion: the empty TUI
   starts and all checks pass.
2. Prove the MPRIS seam with a small diagnostic command that prints normalized
   playback state and exits. Completion: it reports a playing track and reports
   a typed disconnected state when `spotifyd` is absent.
3. Implement `AppState`, events, commands, and a pure reducer before the full UI.
   Completion: tests cover playback changes, track changes, progress
   interpolation, disconnect, and reconnect.
4. Build the responsive text UI and keyboard dispatcher. Completion: every v1
   control works against `spotifyd`, and narrow terminals show a usable fallback
   layout.
5. Add artwork loading, bounded caching, Kitty rendering, and placeholders.
   Completion: changing tracks never leaves stale artwork on screen.
6. Add the `SpectrumSource` interface and `cava` adapter. Completion: the TUI
   remains responsive when `cava` is missing, stopped, or producing malformed
   data.
7. Package the application for the workstation and replace the Hyprland launch
   command only after acceptance. Completion: a clean NixOS rebuild launches the
   new TUI on workspace 10.

## Acceptance checks

Version 1 is complete when all of the following are reproducible:

- Starting the TUI before `spotifyd` shows a disconnected state and later
  reconnects without restarting.
- Playback controls respond without Spotify Web API requests.
- Track metadata, progress, and artwork update correctly across ten consecutive
  track changes.
- Pausing, losing network connectivity, and restarting `spotifyd` do not crash or
  freeze the TUI.
- Missing artwork and missing `cava` degrade cleanly.
- The UI renders correctly in Ghostty at 80x24 and at the normal workspace size.
- Unit tests, formatting, Clippy, and the Nix package build all pass.

## First diagnostic commands

Run these on `stevie` inside the graphical session:

```bash
playerctl -p spotifyd status
playerctl -p spotifyd metadata
busctl --user introspect \
  org.mpris.MediaPlayer2.spotifyd \
  /org/mpris/MediaPlayer2
```

If these do not see a player, verify that `spotifyd` is running and attached to
the session bus before writing application code.
