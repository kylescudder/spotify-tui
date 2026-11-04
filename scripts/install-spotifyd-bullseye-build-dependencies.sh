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

# Snapshot downloads can return transient 503/504 errors. Retry both index and
# package downloads, and never continue with an incomplete package index.
apt-get -o Acquire::Retries=5 -o Acquire::Check-Valid-Until=false \
  -o APT::Update::Error-Mode=any update
DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=5 install --yes \
  binutils \
  libasound2-dev \
  libdbus-1-dev \
  libpulse-dev \
  pkg-config
