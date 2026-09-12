#!/bin/sh

set -eu

repository_root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/spotify-tui-formula-stage-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

checksum=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
published="$test_root/published.rb"
staged="$test_root/staged.rb"
source_url="file://$test_root/spotify-tui-source.tar.gz"

sh "$repository_root/scripts/render-homebrew-formula.sh" \
  example/spotify-tui \
  v1.2.3 \
  "$checksum" \
  "$published"

if grep -Eq '^  version ' "$published"; then
  echo "published formula must not declare a URL-derived version" >&2
  exit 1
fi

sh "$repository_root/scripts/stage-homebrew-formula.sh" \
  "$published" \
  v1.2.3 \
  "$source_url" \
  "$staged"

grep -F "  url \"$source_url\"" "$staged" >/dev/null
grep -F '  version "1.2.3"' "$staged" >/dev/null
grep -F 'https://github.com/Spotifyd/spotifyd/archive/' "$staged" >/dev/null

if [ "$(grep -Ec '^  version ' "$staged")" -ne 1 ]; then
  echo "staged formula must declare exactly one version" >&2
  exit 1
fi

if command -v ruby >/dev/null 2>&1; then
  ruby -c "$staged" >/dev/null
fi

if sh "$repository_root/scripts/stage-homebrew-formula.sh" \
  "$published" \
  invalid-version \
  "$source_url" \
  "$test_root/rejected-version.rb" >/dev/null 2>&1; then
  echo "formula staging accepted an invalid version" >&2
  exit 1
fi

if sh "$repository_root/scripts/stage-homebrew-formula.sh" \
  "$published" \
  1.2.3 \
  https://example.com/spotify-tui-source.tar.gz \
  "$test_root/rejected-url.rb" >/dev/null 2>&1; then
  echo "formula staging accepted a non-local source URL" >&2
  exit 1
fi

echo "Homebrew formula staging tests passed"
