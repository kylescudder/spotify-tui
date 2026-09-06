# Changelog

All notable changes to Spotify TUI will be documented in this file. The project
uses semantic versioning once public releases begin.

## Unreleased

- Initial Ratatui application and deterministic playback state model.
- Linux Spotifyd MPRIS diagnostic and delegated Spotifyd authentication flow.
- Discovery of Spotifyd's process-unique MPRIS bus name, with compatibility for
  the legacy fixed name.
- Live Linux MPRIS property and seek subscriptions, bounded automatic
  reconnection, and play/pause, track, seek, and volume controls in the TUI.
- Fixed a restart race that could miss Spotifyd's MPRIS name disappearing and
  leave the playback supervisor waiting on the terminated player indefinitely.
- Made the private D-Bus restart regression test use its own temporary socket
  and session configuration so it runs deterministically in Nix build sandboxes.
- A live now-playing view with track metadata, interpolated progress, playback
  state, volume, and discoverable arrow-key and Vim-style bindings.
- TOML configuration with built-in and custom colour themes.
- Locked Nix flake, Homebrew formula automation, cross-platform release assets,
  direct POSIX and PowerShell installers, checksums, and build provenance.
- Self-contained direct archives with a pinned Spotifyd runtime, verified
  corresponding source and licence, safe config defaults, and user-level
  startup integration.
