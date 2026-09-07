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

for binary in spotify-tui spotify-tui-diagnose spotifyd; do
  printf '#!/bin/sh\nprintf "fixture %s\\n"\n' "$binary" > "$test_root/stage/$binary"
  chmod 0755 "$test_root/stage/$binary"
done
printf 'fixture Spotifyd licence\n' > "$test_root/stage/SPOTIFYD-LICENSE"

tar -czf "$test_root/release/$artifact" -C "$test_root/stage" \
  spotify-tui spotify-tui-diagnose spotifyd SPOTIFYD-LICENSE
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
    --install-dir "$test_root/bin" \
    --config-dir "$test_root/config" \
    --no-service

test -x "$test_root/bin/spotify-tui"
test -x "$test_root/bin/spotify-tui-diagnose"
test -x "$test_root/bin/spotifyd"
test "$("$test_root/bin/spotify-tui")" = "fixture spotify-tui"
test "$("$test_root/bin/spotifyd")" = "fixture spotifyd"
test -f "$test_root/share/spotify-tui/SPOTIFYD-LICENSE"
grep -F 'volume_controller = "softvol"' "$test_root/config/spotifyd.conf" >/dev/null
grep -F 'initial_volume = 90' "$test_root/config/spotifyd.conf" >/dev/null
if [ "$os" = unknown-linux-musl ]; then
  grep -F 'use_mpris = true' "$test_root/config/spotifyd.conf" >/dev/null
  grep -F 'dbus_type = "session"' "$test_root/config/spotifyd.conf" >/dev/null
fi

printf '#!/bin/sh\nprintf "upgraded spotify-tui\\n"\n' > "$test_root/stage/spotify-tui"
chmod 0755 "$test_root/stage/spotify-tui"
printf '#!/bin/sh\nprintf "upgraded spotifyd\\n"\n' > "$test_root/stage/spotifyd"
chmod 0755 "$test_root/stage/spotifyd"
tar -czf "$test_root/release/$artifact" -C "$test_root/stage" \
  spotify-tui spotify-tui-diagnose spotifyd SPOTIFYD-LICENSE
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
    --install-dir "$test_root/bin" \
    --config-dir "$test_root/config" \
    --no-service >/dev/null
test "$("$test_root/bin/spotify-tui")" = "upgraded spotify-tui"
if [ "$os" = apple-darwin ]; then
  test "$("$test_root/bin/spotifyd")" = "upgraded spotifyd"
else
  test "$("$test_root/bin/spotifyd")" = "fixture spotifyd"
fi

SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  sh "$repository_root/scripts/install.sh" \
    --repository example/spotify-tui \
    --install-dir "$test_root/bin" \
    --config-dir "$test_root/config" \
    --force-dependencies \
    --no-service >/dev/null
test "$("$test_root/bin/spotifyd")" = "upgraded spotifyd"

SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  sh "$repository_root/scripts/install.sh" \
    --repository example/spotify-tui \
    --install-dir "$test_root/no-dependencies/bin" \
    --config-dir "$test_root/no-dependencies/config" \
    --no-dependencies \
    --no-service >/dev/null
test ! -e "$test_root/no-dependencies/bin/spotifyd"
test ! -e "$test_root/no-dependencies/config/spotifyd.conf"

if [ "$os" = unknown-linux-musl ]; then
  service_program="$test_root/fake-systemctl"
  cat > "$service_program" <<EOF
#!/bin/sh
printf '%s\\n' "\$*" >> "$test_root/service-calls"
if [ "\${2:-}" = cat ]; then
  exit 1
fi
EOF
  chmod 0755 "$service_program"
  SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
  SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  SPOTIFY_TUI_SYSTEMCTL="$service_program" \
  SPOTIFY_TUI_SYSTEMD_USER_DIR="$test_root/service/systemd" \
    sh "$repository_root/scripts/install.sh" \
      --repository example/spotify-tui \
      --install-dir "$test_root/service/bin" \
      --config-dir "$test_root/service/config" >/dev/null
  grep -F "ExecStart=\"$test_root/service/bin/spotifyd\" --config-path \"$test_root/service/config/spotifyd.conf\" --no-daemon" \
    "$test_root/service/systemd/spotifyd.service" >/dev/null
  grep -F -- '--user daemon-reload' "$test_root/service-calls" >/dev/null
  grep -F -- '--user enable --now spotifyd.service' "$test_root/service-calls" >/dev/null
else
  service_program="$test_root/fake-launchctl"
  cat > "$service_program" <<EOF
#!/bin/sh
printf '%s\\n' "\$*" >> "$test_root/service-calls"
if [ "\${1:-}" = print ]; then
  exit 1
fi
EOF
  chmod 0755 "$service_program"
  SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
  SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  SPOTIFY_TUI_LAUNCHCTL="$service_program" \
  SPOTIFY_TUI_LAUNCH_AGENTS_DIR="$test_root/service/launch-agents" \
    sh "$repository_root/scripts/install.sh" \
      --repository example/spotify-tui \
      --install-dir "$test_root/service/bin" \
      --config-dir "$test_root/service/config" >/dev/null
  grep -F "<string>$test_root/service/bin/spotifyd</string>" \
    "$test_root/service/launch-agents/io.github.kylescudder.spotifyd.plist" >/dev/null
  grep -F "<string>$test_root/service/config/spotifyd.conf</string>" \
    "$test_root/service/launch-agents/io.github.kylescudder.spotifyd.plist" >/dev/null
  grep -F 'bootstrap gui/' "$test_root/service-calls" >/dev/null
fi

printf '%064d  %s\n' 0 "$artifact" > "$test_root/release/SHA256SUMS"
if SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
  SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  sh "$repository_root/scripts/install.sh" \
    --repository example/spotify-tui \
    --install-dir "$test_root/rejected" \
    --no-dependencies \
    --no-service >/dev/null 2>&1; then
  echo "installer accepted a bad checksum" >&2
  exit 1
fi

if SPOTIFY_TUI_RELEASE_BASE_URL="$test_root/release" \
  SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS=1 \
  sh "$repository_root/scripts/install.sh" \
    --repository example/spotify-tui \
    --version definitely-not-semver \
    --install-dir "$test_root/rejected-version" \
    --no-dependencies \
    --no-service >/dev/null 2>&1; then
  echo "installer accepted an invalid version" >&2
  exit 1
fi

echo "POSIX installer tests passed"
