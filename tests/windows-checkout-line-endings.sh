#!/bin/sh

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT HUP INT TERM
carriage_return=$(printf '\r')

git -C "$repo_root" -c core.autocrlf=true checkout-index \
  --prefix="$fixture_dir/" -- \
  packaging/spotifyd/apply-local-control.sh \
  packaging/spotifyd/spotifyd.patch

for path in \
  packaging/spotifyd/apply-local-control.sh \
  packaging/spotifyd/spotifyd.patch
do
  if grep -q "$carriage_return" "$fixture_dir/$path"; then
    echo "Windows checkout converted $path to CRLF" >&2
    exit 1
  fi
done

echo "Windows checkout preserves LF for Spotifyd patch inputs"
