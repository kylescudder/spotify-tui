# Changelog

All notable changes to Spotify TUI will be documented in this file. The project
uses semantic versioning once public releases begin.

## Unreleased

- Initial Ratatui application and deterministic playback state model.
- Phone-free Linux startup through Spotifyd's local D-Bus transfer control,
  including automatic recovery after daemon restarts and an optional configured
  startup Spotify URI.
- Automatic cross-platform Spotifyd lifecycle management on TUI launch, retry,
  and post-authentication restart, with Linux systemd/transient-unit, macOS
  launchd/Homebrew, Windows process, and detached fallback adapters.
- Linux Spotifyd MPRIS diagnostic and delegated Spotifyd authentication flow.
- Discovery of Spotifyd's process-unique MPRIS bus name, with compatibility for
  the legacy fixed name.
- Live Linux MPRIS property and seek subscriptions, bounded automatic
  reconnection, and play/pause, track, seek, and volume controls in the TUI.
- Added an authenticated loopback playback adapter for macOS and Windows,
  backed by the pinned bundled Spotifyd runtime, with per-user endpoint
  discovery, phone-free activation, metadata, artwork, progress, every playback
  command, and automatic recovery after daemon restarts.
- Fixed a restart race that could miss Spotifyd's MPRIS name disappearing and
  leave the playback supervisor waiting on the terminated player indefinitely.
- Treat Spotifyd's transient "no position available currently" MPRIS response
  as position zero instead of discarding otherwise valid playback state.
- Made the private D-Bus restart regression test use its own temporary socket
  and session configuration so it runs deterministically in Nix build sandboxes.
- A live now-playing view with track metadata, interpolated progress, playback
  state, volume, and discoverable arrow-key and Vim-style bindings.
- Asynchronous album artwork from Spotifyd metadata with HTTPS, download and
  decode limits, a bounded in-memory cache, stale-track protection, Ghostty
  Kitty graphics, Unicode half-block fallback, clean borderless presentation,
  bounded responsive placement, vertically centred compact playback details,
  and placeholders that clear once artwork is ready.
- TOML configuration with built-in and custom colour themes.
- Optional Spotify Web API catalogue access using a user-supplied client ID,
  browser-based PKCE authentication, an owner-only token cache, and automatic
  refresh without a client secret.
- Launch catalogue authentication lazily from the first search and resume the
  pending request after browser approval without leaving or restarting the TUI.
- Keyboard-first search across artists, albums, tracks, and playlists, with
  artist release pages, album track pages, navigation history, asynchronous
  loading, and local Spotifyd playback for selected content.
- Contextual artist and album imagery in responsive catalogue views, using the
  existing bounded asynchronous artwork cache and narrow-layout fallback.
- Artist-page artwork now follows the selected release, falling back to the
  artist image, while a separate speculative worker prefetches nearby images
  into the shared LRU cache without blocking foreground artwork.
- Cached artwork now bypasses the worker queue and is applied before drawing the
  next frame; pending downloads and terminal encoding are checked every 16 ms
  instead of waiting for the 250 ms idle input tick.
- Start catalogue tracks through Spotify playback with an exact URI offset,
  working around Spotifyd 0.4.2's one-based/zero-based MPRIS `OpenUri` bug while
  retaining the selected album as the playback context.
- Fall back to the same selected track through local Spotifyd control when the
  Web API temporarily returns `404` because it cannot see an active device,
  instead of replacing the player with a fatal catalogue error.
- Added deterministic catalogue failure coverage for actionable Spotify Web API
  `403` permission and `429` quota responses.
- Removed the planned audio spectrum from the product scope and dropped the
  unused `cava` dependency from Nix and Homebrew packaging.
- Locked Nix flake, Homebrew formula automation, cross-platform release assets,
  direct POSIX and PowerShell installers, checksums, and build provenance.
- Bound public and per-platform release approvals to the exact Cargo package
  version so a version bump automatically invalidates stale readiness state.
- Added a Balm-style, write-scoped Homebrew tap deploy key, an explicit
  content-neutral production-branch write check, release serialization, and a
  downgrade guard. Tested assets are now published before the formula is
  exposed, so Homebrew never points at a missing archive.
- Preserve LF endings for shell scripts and patches during Windows checkouts so
  the patched Spotifyd release build applies cleanly.
- Self-contained direct archives with a pinned Spotifyd runtime, verified
  corresponding source and licence, safe config defaults, and user-level
  startup integration that starts the daemon during installation.
- A Homebrew Spotifyd service and Nix wrapper support for zero-command daemon
  startup outside Home Manager.
