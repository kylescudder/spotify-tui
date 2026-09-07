# Spotifyd local-control patch

Stock Spotifyd 0.4.2 exposes local playback control through D-Bus/MPRIS, which
is not available in its portable macOS and Windows builds. Release packaging
applies `spotifyd.patch`, copies `local_control.rs`, and builds the pinned
Spotifyd source with `rodio_backend,local_control` on those platforms. Linux
continues to use the unmodified upstream MPRIS release.

The feature binds an ephemeral loopback TCP port and writes its address and a
random 256-bit token beneath the platform's per-user local-data directory in
`spotify-tui/spotifyd-control-address` and
`spotify-tui/spotifyd-control-token`. Every newline-delimited JSON request must
carry protocol version `1` and that token. Connections from non-loopback
addresses are ignored, request and response I/O is bounded, and token comparison
does not short-circuit over equal-length values.

The request operations are `snapshot`, `activate`, `toggle`, `previous`, `next`,
`seek_by`, `set_volume`, and `open_uri`. `src/local_control.rs` in the Spotify
TUI source is the matching client and owns conversion into the platform-neutral
`PlaybackSource` model. It rereads endpoint discovery for every request so a
Spotifyd restart and new ephemeral port recover without restarting the TUI.

For controlled tests, both processes accept these overrides:

- `SPOTIFY_TUI_CONTROL_ADDRESS`
- `SPOTIFY_TUI_CONTROL_ADDRESS_FILE`
- `SPOTIFY_TUI_CONTROL_TOKEN_FILE`

`spotifyd.patch` intentionally targets commit
`c5b94367014856a8c541dea565cbd332e034fb9e` (Spotifyd 0.4.2). When updating the
pin, reapply the patch to a clean checkout, compile the `local_control` feature,
run its test, and repeat native macOS and Windows acceptance. Modified Spotifyd
binaries remain GPL-3.0-only; releases publish the pinned complete upstream
source and include this patch as corresponding source.
