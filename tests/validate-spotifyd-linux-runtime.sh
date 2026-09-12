#!/bin/sh

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
validator="$repo_root/scripts/validate-spotifyd-linux-runtime.sh"
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT HUP INT TERM

fail() {
  echo "Spotifyd Linux runtime validator test failed: $*" >&2
  exit 1
}

[ -f "$validator" ] || fail "missing runtime dependency validator"

touch "$fixture_dir/spotifyd"

cat >"$fixture_dir/readelf-openssl-1.1" <<'EOF'
#!/bin/sh
cat <<'OUTPUT'
 0x0000000000000001 (NEEDED) Shared library: [libssl.so.1.1]
 0x0000000000000001 (NEEDED) Shared library: [libcrypto.so.1.1]
OUTPUT
EOF
chmod 0755 "$fixture_dir/readelf-openssl-1.1"

if SPOTIFY_TUI_READELF="$fixture_dir/readelf-openssl-1.1" \
  sh "$validator" "$fixture_dir/spotifyd" >/dev/null 2>&1; then
  fail "an OpenSSL 1.1-linked runtime must be rejected"
fi

cat >"$fixture_dir/readelf-openssl-3" <<'EOF'
#!/bin/sh
cat <<'OUTPUT'
 0x0000000000000001 (NEEDED) Shared library: [libssl.so.3]
 0x0000000000000001 (NEEDED) Shared library: [libcrypto.so.3]
OUTPUT
EOF
chmod 0755 "$fixture_dir/readelf-openssl-3"

SPOTIFY_TUI_READELF="$fixture_dir/readelf-openssl-3" \
  sh "$validator" "$fixture_dir/spotifyd" >/dev/null ||
  fail "an OpenSSL 3-linked runtime should pass"

cat >"$fixture_dir/readelf-new-glibc" <<'EOF'
#!/bin/sh
cat <<'OUTPUT'
  0x069691b9 0x00 12 GLIBC_2.39
OUTPUT
EOF
chmod 0755 "$fixture_dir/readelf-new-glibc"

if SPOTIFY_TUI_READELF="$fixture_dir/readelf-new-glibc" \
  sh "$validator" "$fixture_dir/spotifyd" >/dev/null 2>&1; then
  fail "a runtime requiring glibc newer than 2.31 must be rejected"
fi

cat >"$fixture_dir/readelf-baseline-glibc" <<'EOF'
#!/bin/sh
cat <<'OUTPUT'
  0x069691b4 0x00 11 GLIBC_2.28
  0x069691b7 0x00 10 GLIBC_2.31
OUTPUT
EOF
chmod 0755 "$fixture_dir/readelf-baseline-glibc"

SPOTIFY_TUI_READELF="$fixture_dir/readelf-baseline-glibc" \
  sh "$validator" "$fixture_dir/spotifyd" >/dev/null ||
  fail "a runtime within the glibc 2.31 baseline should pass"

cat >"$fixture_dir/readelf-glibc-2.32" <<'EOF'
#!/bin/sh
cat <<'OUTPUT'
  0x069691b8 0x00 11 GLIBC_2.32
OUTPUT
EOF
chmod 0755 "$fixture_dir/readelf-glibc-2.32"

if SPOTIFY_TUI_READELF="$fixture_dir/readelf-glibc-2.32" \
  SPOTIFY_TUI_MAX_GLIBC=2.31 \
  sh "$validator" "$fixture_dir/spotifyd" >/dev/null 2>&1; then
  fail "the configured glibc baseline must be enforced"
fi

if SPOTIFY_TUI_READELF="$fixture_dir/readelf-baseline-glibc" \
  SPOTIFY_TUI_MAX_GLIBC=not-a-version \
  sh "$validator" "$fixture_dir/spotifyd" >/dev/null 2>&1; then
  fail "an invalid glibc baseline must be rejected"
fi

cat >"$fixture_dir/readelf-static" <<'EOF'
#!/bin/sh
cat <<'OUTPUT'
There is no dynamic section in this file.
OUTPUT
EOF
chmod 0755 "$fixture_dir/readelf-static"

SPOTIFY_TUI_READELF="$fixture_dir/readelf-static" \
  sh "$validator" "$fixture_dir/spotifyd" >/dev/null ||
  fail "a runtime without obsolete dynamic OpenSSL dependencies should pass"

cat >"$fixture_dir/readelf-failure" <<'EOF'
#!/bin/sh
exit 1
EOF
chmod 0755 "$fixture_dir/readelf-failure"

if SPOTIFY_TUI_READELF="$fixture_dir/readelf-failure" \
  sh "$validator" "$fixture_dir/spotifyd" >/dev/null 2>&1; then
  fail "an unreadable runtime must not pass validation"
fi

echo "Spotifyd Linux runtime validator checks passed"
