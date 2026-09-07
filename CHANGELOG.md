# Changelog

All notable changes to Spotify TUI will be documented in this file. The project
uses semantic versioning once public releases begin.

## Unreleased

- Initial Ratatui application and deterministic playback state model.
- Phone-free Linux startup through Spotifyd's local D-Bus transfer control,
  including automatic recovery after daemon restarts and an optional configured
  startup Spotify URI.
- Linux Spotifyd MPRIS diagnostic and delegated Spotifyd authentication flow.
- Discovery of Spotifyd's process-unique MPRIS bus name, with compatibility for
  the legacy fixed name.
- Live Linux MPRIS property and seek subscriptions, bounded automatic
  reconnection, and play/pause, track, seek, and volume controls in the TUI.
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
- Start catalogue tracks through Spotify playback with an exact URI offset,
  working around Spotifyd 0.4.2's one-based/zero-based MPRIS `OpenUri` bug while
  retaining the selected album as the playback context.
- Removed the planned audio spectrum from the product scope and dropped the
  unused `cava` dependency from Nix and Homebrew packaging.
- Locked Nix flake, Homebrew formula automation, cross-platform release assets,
  direct POSIX and PowerShell installers, checksums, and build provenance.
- Self-contained direct archives with a pinned Spotifyd runtime, verified
  corresponding source and licence, safe config defaults, and user-level
  startup integration.
