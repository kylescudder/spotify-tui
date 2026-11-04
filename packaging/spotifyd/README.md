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

## ARM64 Linux build dependencies

The CI and release jobs build in the digest-pinned Rust 1.88.0 Bullseye image
specified in their workflows. `bullseye-arm64-build-dependencies.sha256` locks
the additional Debian packages needed for ALSA, PulseAudio, and D-Bus. Binutils
and pkg-config are already present in that image.

`scripts/install-spotifyd-bullseye-build-dependencies.sh` downloads those exact
package files from Debian's regular pool, falling back to its archive pool,
checks every SHA-256 hash, then installs the complete set with `dpkg`. Installation
requires no package indexes and cannot fetch additional dependencies. This avoids
the snapshot service's index and package redirect outages while retaining the
versions from the `20250724T000000Z` snapshot.

To update the lock, obtain `dists/bullseye/InRelease` and
`dists/bullseye/main/binary-arm64/Packages.xz` from the intended Debian snapshot.
Verify the InRelease signatures with `gpgv` and the pinned image's Debian archive
keyring, then verify the compressed index's SHA-256 against that signed release.
Copy each required package's `SHA256` and `Filename` fields into the lock. Review
the complete dependency set against a clean instance of the pinned ARM64 image.
When changing the base image or snapshot, repeat this process and verify installation
with `/var/lib/apt/lists` empty, build the pinned Spotifyd source, and run
`scripts/validate-spotifyd-linux-runtime.sh` with `SPOTIFY_TUI_MAX_GLIBC=2.31`.
