#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
  echo "usage: package-unix.sh TARGET VERSION OUTPUT_DIRECTORY" >&2
  exit 2
fi

target=$1
version=${2#v}
output_dir=$3
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

cp "$repository_root/README.md" "$repository_root/LICENSE" "$stage_dir/"
mkdir "$stage_dir/themes"
cp "$repository_root"/themes/*.toml "$stage_dir/themes/"

mkdir -p "$output_dir"
archive="$output_dir/spotify-tui-${target}.tar.gz"
tar -czf "$archive" -C "$stage_dir" \
  spotify-tui spotify-tui-diagnose README.md LICENSE themes
echo "$archive"
