#!/bin/sh

set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: apply-local-control.sh SPOTIFYD_SOURCE [LIBRESPOT_CONNECT_ARCHIVE]" >&2
  exit 1
fi

spotifyd_source=$(CDPATH= cd -- "$1" && pwd)
# A source archive may be unpacked inside another checkout. Stop git discovery
# at its parent so git apply does not silently skip the nested source tree.
GIT_CEILING_DIRECTORIES=$(dirname "$spotifyd_source")
export GIT_CEILING_DIRECTORIES
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

git -C "$spotifyd_source" apply --recount --no-index "$script_dir/spotifyd.patch"
cp "$script_dir/local_control.rs" "$spotifyd_source/src/local_control.rs"

# Vendor only the pinned connect crate so local queue access uses the existing
# Connect state machine; release corresponding source includes this directory.
connect_source="$spotifyd_source/spotify-tui-librespot-connect"
archive="$spotifyd_source/librespot-connect-0.8.0.crate"
if [ "$#" -eq 2 ]; then
  cp "$2" "$archive"
else
  curl --fail --location --silent --show-error \
    https://static.crates.io/crates/librespot-connect/librespot-connect-0.8.0.crate \
    --output "$archive"
fi
expected=4b2a21e9e19b906c6676b1a515fc5529d5414f69049f5a167dfb2ea3206e5a27
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$archive" | cut -d ' ' -f 1)
else
  actual=$(shasum -a 256 "$archive" | cut -d ' ' -f 1)
fi
[ "$actual" = "$expected" ] || { echo "librespot-connect checksum mismatch" >&2; exit 1; }
mkdir "$connect_source"
tar -xzf "$archive" --strip-components=1 -C "$connect_source"
rm "$archive"
git -C "$spotifyd_source" apply --no-index --directory=spotify-tui-librespot-connect "$script_dir/librespot-queue.patch"
cp "$script_dir/queue_control.rs" "$connect_source/src/queue_control.rs"
cat >> "$spotifyd_source/Cargo.toml" <<'EOF'

[patch.crates-io]
librespot-connect = { path = "spotify-tui-librespot-connect" }
EOF
# Path patches retain the upstream locked dependency graph, without the registry
# source/checksum on the one locally patched package. Builds remain --locked.
awk '
  /^\[\[package\]\]$/ { connect = 0 }
  /^name = "librespot-connect"$/ { connect = 1 }
  connect && /^(source|checksum) = / { next }
  { print }
' "$spotifyd_source/Cargo.lock" > "$spotifyd_source/Cargo.lock.queue"
mv "$spotifyd_source/Cargo.lock.queue" "$spotifyd_source/Cargo.lock"
