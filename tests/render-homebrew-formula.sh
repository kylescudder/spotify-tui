#!/bin/sh
set -eu

repository_root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/spotify-tui-formula-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

checksum=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
output="$test_root/spotify-tui.rb"

sh "$repository_root/scripts/render-homebrew-formula.sh" \
  example/spotify-tui \
  v1.2.3 \
  "$checksum" \
  "$output"

grep -F 'homepage "https://github.com/example/spotify-tui"' "$output" >/dev/null
grep -F 'releases/download/v1.2.3/spotify-tui-source.tar.gz' "$output" >/dev/null
grep -F "sha256 \"$checksum\"" "$output" >/dev/null
grep -F 'service do' "$output" >/dev/null
grep -F 'Formula["spotifyd"].opt_bin/"spotifyd"' "$output" >/dev/null
grep -F 'keep_alive true' "$output" >/dev/null

if grep -E '@(REPOSITORY|VERSION|SOURCE_SHA256)@' "$output" >/dev/null; then
  echo "rendered formula still contains placeholders" >&2
  exit 1
fi

if command -v ruby >/dev/null 2>&1; then
  ruby -c "$output" >/dev/null
fi

if sh "$repository_root/scripts/render-homebrew-formula.sh" \
  invalid \
  1.2.3 \
  "$checksum" \
  "$test_root/rejected.rb" >/dev/null 2>&1; then
  echo "renderer accepted an invalid repository" >&2
  exit 1
fi

if sh "$repository_root/scripts/render-homebrew-formula.sh" \
  example/spotify-tui \
  invalid-version \
  "$checksum" \
  "$test_root/rejected-version.rb" >/dev/null 2>&1; then
  echo "renderer accepted an invalid version" >&2
  exit 1
fi

echo "Homebrew formula renderer tests passed"
