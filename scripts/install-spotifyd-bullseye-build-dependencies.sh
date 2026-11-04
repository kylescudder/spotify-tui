#!/bin/sh

set -eu

snapshot_timestamp=20250724T000000Z
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
manifest="$script_dir/../packaging/spotifyd/bullseye-arm64-build-dependencies.sha256"

if [ "$(dpkg --print-architecture)" != arm64 ]; then
  echo 'the locked Spotifyd Bullseye dependencies require ARM64' >&2
  exit 1
fi

archives=$(mktemp -d)
trap 'rm -rf "$archives"' EXIT HUP INT TERM

echo "Installing Spotifyd dependencies locked to Debian snapshot $snapshot_timestamp"
# The package filenames and hashes come from the signed snapshot index. Fetch
# those exact files from Debian's pool mirrors without querying snapshot indexes.
while read -r checksum package; do
  case "$checksum" in ''|\#*) continue ;; esac
  archive="$archives/${package##*/}"
  downloaded=false
  for mirror in https://deb.debian.org/debian https://archive.debian.org/debian; do
    if curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
      --retry 3 --connect-timeout 15 --max-time 120 \
      "$mirror/$package" -o "$archive" &&
      printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check --status; then
      downloaded=true
      break
    fi
    rm -f "$archive"
  done
  if [ "$downloaded" != true ]; then
    echo "could not download and verify locked dependency: $package" >&2
    exit 1
  fi
done <"$manifest"

# Unpack the complete locked set before dpkg configures its dependencies. dpkg
# never fetches packages or indexes; unresolved dependencies fail the build.
dpkg --install "$archives/"*.deb
pkg-config --exists alsa dbus-1 libpulse
readelf --version >/dev/null
