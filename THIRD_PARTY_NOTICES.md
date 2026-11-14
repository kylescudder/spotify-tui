# Third-party notices

Direct-release archives bundle Spotifyd 0.4.2, built from commit
`c5b94367014856a8c541dea565cbd332e034fb9e`. Spotifyd is licensed under the GNU
General Public License version 3. Its licence is included as
`SPOTIFYD-LICENSE`, and the complete corresponding source is published beside
each Spotify TUI release as `spotifyd-0.4.2-source.tar.gz`. The build patches
and locked dependency changes applied to that source are included in the
Spotify TUI source archive under `packaging/spotifyd`.

Spotifyd source and project information:
https://github.com/Spotifyd/spotifyd/tree/v0.4.2

The local queue-control extension patches librespot-connect 0.8.0 (MIT).
Its original source is published as `librespot-connect-0.8.0-source.crate` beside
each release. The checksum, patch, and added module are in `packaging/spotifyd`;
`apply-local-control.sh` reconstructs the modified source without changing
upstream dependency versions.
