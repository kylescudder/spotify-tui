#!/bin/sh

set -eu

snapshot_timestamp=20250724T000000Z
debian_snapshot="https://snapshot.debian.org/archive/debian/$snapshot_timestamp"
security_snapshot="https://snapshot.debian.org/archive/debian-security/$snapshot_timestamp"
sources=/etc/apt/sources.list

sed -i \
  -e "s|http://deb.debian.org/debian-security|$security_snapshot|g" \
  -e "s|http://security.debian.org/debian-security|$security_snapshot|g" \
  -e "s|http://deb.debian.org/debian|$debian_snapshot|g" \
  "$sources"

if grep -Eq 'deb\.debian\.org|security\.debian\.org' "$sources"; then
  echo "could not pin every Debian package source to the build snapshot" >&2
  exit 1
fi

install_dependencies() {
  # Retry index and package downloads, and stop on an incomplete package index.
  # Return explicitly: this function is also called from an if condition, where
  # the shell's errexit would otherwise allow installation after update failed.
  apt-get -o Acquire::Retries=5 -o Acquire::Check-Valid-Until=false \
    -o APT::Update::Error-Mode=any update || return "$?"
  DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=5 install --yes \
    binutils \
    libasound2-dev \
    libdbus-1-dev \
    libpulse-dev \
    pkg-config
}

if install_dependencies; then
  exit 0
fi

# A CDN outage can outlast the per-file retries. The alternate Debian frontend
# serves the same immutable snapshot; APT still verifies signatures and hashes.
echo "Snapshot download failed; trying Debian's alternate snapshot frontend" >&2
sed -i 's|https://snapshot.debian.org/|https://snapshot-cloudflare.debian.org/|g' "$sources"
install_dependencies
