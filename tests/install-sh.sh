#!/bin/sh
set -eu

repository_root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/spotify-tui-installer-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

case "$(uname -m)" in
  x86_64|amd64) architecture=x86_64 ;;
  arm64|aarch64) architecture=aarch64 ;;
  *) echo "unsupported test architecture" >&2; exit 1 ;;
esac

case "$(uname -s)" in
  Linux) os=unknown-linux-musl ;;
  Darwin) os=apple-darwin ;;
  *) echo "unsupported test operating system" >&2; exit 1 ;;
esac

artifact="spotify-tui-${architecture}-${os}.tar.gz"
mkdir "$test_root/release" "$test_root/stage"

for binary in spotify-tui spotify-tui-diagnose; do
  printf '#!/bin/sh\nprintf "fixture %s\\n"\n' "$binary" > "$test_root/stage/$binary"
  chmod 0755 "$test_root/stage/$binary"
done

tar -czf "$test_root/release/$artifact" -C "$test_root/stage" spotify-tui spotify-tui-diagnose
if command -v sha256sum >/dev/null 2>&1; then
  checksum=$(sha256sum "$test_root/release/$artifact" | awk '{ print $1 }')
else
  checksum=$(shasum -a 256 "$test_root/release/$artifact" | awk '{ print $1 }')
fi
printf '%s  %s\n' "$checksum" "$artifact" > "$test_root/release/SHA256SUMS"

SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  sh "$repository_root/scripts/install.sh" \
    --repository example/spotify-tui \
    --install-dir "$test_root/bin"

test -x "$test_root/bin/spotify-tui"
test -x "$test_root/bin/spotify-tui-diagnose"
test "$("$test_root/bin/spotify-tui")" = "fixture spotify-tui"

printf '#!/bin/sh\nprintf "upgraded spotify-tui\\n"\n' > "$test_root/stage/spotify-tui"
chmod 0755 "$test_root/stage/spotify-tui"
tar -czf "$test_root/release/$artifact" -C "$test_root/stage" spotify-tui spotify-tui-diagnose
if command -v sha256sum >/dev/null 2>&1; then
  checksum=$(sha256sum "$test_root/release/$artifact" | awk '{ print $1 }')
else
  checksum=$(shasum -a 256 "$test_root/release/$artifact" | awk '{ print $1 }')
fi
printf '%s  %s\n' "$checksum" "$artifact" > "$test_root/release/SHA256SUMS"

SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  sh "$repository_root/scripts/install.sh" \
    --repository example/spotify-tui \
    --version 1.2.3 \
    --install-dir "$test_root/bin" >/dev/null
test "$("$test_root/bin/spotify-tui")" = "upgraded spotify-tui"

printf '%064d  %s\n' 0 "$artifact" > "$test_root/release/SHA256SUMS"
if SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
  SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  sh "$repository_root/scripts/install.sh" \
    --repository example/spotify-tui \
    --install-dir "$test_root/rejected" >/dev/null 2>&1; then
  echo "installer accepted a bad checksum" >&2
  exit 1
fi

if SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
  SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  sh "$repository_root/scripts/install.sh" \
    --repository example/spotify-tui \
    --version definitely-not-semver \
    --install-dir "$test_root/rejected-version" >/dev/null 2>&1; then
  echo "installer accepted an invalid version" >&2
  exit 1
fi

echo "POSIX installer tests passed"
