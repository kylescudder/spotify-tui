#!/bin/sh
set -eu

if [ "$#" -ne 5 ]; then
  echo "usage: package-unix.sh TARGET VERSION OUTPUT_DIRECTORY SPOTIFYD_BINARY SPOTIFYD_LICENSE" >&2
  exit 2
fi

target=$1
version=${2#v}
output_dir=$3
spotifyd_binary=$4
spotifyd_license=$5
repository_root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
build_dir="$repository_root/target/$target/release"
stage_dir=$(mktemp -d "${TMPDIR:-/tmp}/spotify-tui-package.XXXXXX")
trap 'rm -rf "$stage_dir"' EXIT HUP INT TERM

case "$target" in
  x86_64-unknown-linux-musl|aarch64-unknown-linux-musl|x86_64-apple-darwin|aarch64-apple-darwin) ;;
  *) echo "unsupported release target: $target" >&2; exit 2 ;;
esac

if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$'; then
  echo "invalid semantic version: $version" >&2
  exit 2
fi

for binary in spotify-tui spotify-tui-diagnose; do
  if [ ! -x "$build_dir/$binary" ]; then
    echo "missing release binary: $build_dir/$binary" >&2
    exit 1
  fi
  cp "$build_dir/$binary" "$stage_dir/$binary"
done

if [ ! -f "$spotifyd_binary" ]; then
  echo "missing Spotifyd release binary: $spotifyd_binary" >&2
  exit 1
fi
if [ ! -f "$spotifyd_license" ]; then
  echo "missing Spotifyd licence: $spotifyd_license" >&2
  exit 1
fi
cp "$spotifyd_binary" "$stage_dir/spotifyd"
chmod 0755 "$stage_dir/spotifyd"
cp "$spotifyd_license" "$stage_dir/SPOTIFYD-LICENSE"

cp "$repository_root/README.md" "$repository_root/LICENSE" \
  "$repository_root/THIRD_PARTY_NOTICES.md" "$stage_dir/"
mkdir "$stage_dir/themes"
cp "$repository_root"/themes/*.toml "$stage_dir/themes/"

mkdir -p "$output_dir"
archive="$output_dir/spotify-tui-${target}.tar.gz"
tar -czf "$archive" -C "$stage_dir" \
  spotify-tui spotify-tui-diagnose spotifyd README.md LICENSE \
  SPOTIFYD-LICENSE THIRD_PARTY_NOTICES.md themes
echo "$archive"
