#!/bin/sh

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT HUP INT TERM
mkdir -p "$fixture_dir/bin" "$fixture_dir/scripts" "$fixture_dir/packaging/spotifyd" "$fixture_dir/packages"
cp "$repo_root/scripts/install-spotifyd-bullseye-build-dependencies.sh" "$fixture_dir/scripts/"
# Exercise the real download/checksum/install code with small locked artifacts.
printf '# Fixture package lock\n' >"$fixture_dir/packaging/spotifyd/bullseye-arm64-build-dependencies.sha256"
for package in first second; do
  printf '%s package\n' "$package" >"$fixture_dir/packages/$package.deb"
  checksum=$(shasum -a 256 "$fixture_dir/packages/$package.deb" | cut -d ' ' -f 1)
  printf '%s  pool/main/test/%s.deb\n' "$checksum" "$package" \
    >>"$fixture_dir/packaging/spotifyd/bullseye-arm64-build-dependencies.sha256"
done
cat >"$fixture_dir/bin/curl" <<'MOCK'
#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) output=$2; shift 2 ;;
    https:*) url=$1; shift ;;
    *) shift ;;
  esac
done
printf '%s\n' "$url" >>"$DEPENDENCY_CALLS"
case "$DEPENDENCY_SCENARIO:$url" in
  unavailable:*|fallback:https://deb.debian.org/*) exit 22 ;;
  corrupt:*|corrupt-primary:https://deb.debian.org/*) echo 'corrupt package' >"$output" ;;
  *) cp "$DEPENDENCY_PACKAGES/${url##*/}" "$output" ;;
esac
MOCK
cat >"$fixture_dir/bin/dpkg" <<'MOCK'
#!/bin/sh
if [ "$1" = --print-architecture ]; then
  echo "${DEPENDENCY_ARCH:-arm64}"
  exit 0
fi
[ "$1" = --install ] && [ "$#" = 3 ] || exit 1
printf 'install\n' >>"$DEPENDENCY_CALLS"
[ "$DEPENDENCY_SCENARIO" != install-failure ]
MOCK
for tool in pkg-config readelf; do
  printf '#!/bin/sh\nexit 0\n' >"$fixture_dir/bin/$tool"
done
# macOS and Linux must exercise an actual checksum implementation.
printf '#!/bin/sh\nexec shasum -a 256 "$@"\n' >"$fixture_dir/bin/sha256sum"
chmod +x "$fixture_dir/bin/"*
export DEPENDENCY_PACKAGES="$fixture_dir/packages"
export DEPENDENCY_CALLS="$fixture_dir/calls"

for scenario in success fallback corrupt-primary; do
  export DEPENDENCY_SCENARIO=$scenario
  : >"$DEPENDENCY_CALLS"
  PATH="$fixture_dir/bin:$PATH" sh "$fixture_dir/scripts/install-spotifyd-bullseye-build-dependencies.sh" >"$fixture_dir/output" 2>&1
  [ "$(grep -c '^install$' "$DEPENDENCY_CALLS")" = 1 ]
  if [ "$scenario" = success ]; then
    [ "$(wc -l <"$DEPENDENCY_CALLS" | tr -d ' ')" = 3 ]
  else
    [ "$(grep -c '^https://archive.debian.org/' "$DEPENDENCY_CALLS")" = 2 ]
  fi
done

for scenario in unavailable corrupt install-failure; do
  export DEPENDENCY_SCENARIO=$scenario
  : >"$DEPENDENCY_CALLS"
  if PATH="$fixture_dir/bin:$PATH" sh "$fixture_dir/scripts/install-spotifyd-bullseye-build-dependencies.sh" >"$fixture_dir/output" 2>&1; then
    echo "locked dependency installation must reject $scenario" >&2
    exit 1
  fi
  if [ "$scenario" != install-failure ] && grep -q '^install$' "$DEPENDENCY_CALLS"; then
    echo 'unavailable or corrupt packages must never reach dpkg' >&2
    exit 1
  fi
done

export DEPENDENCY_ARCH=amd64
: >"$DEPENDENCY_CALLS"
if PATH="$fixture_dir/bin:$PATH" sh "$fixture_dir/scripts/install-spotifyd-bullseye-build-dependencies.sh" >"$fixture_dir/output" 2>&1; then
  echo 'ARM64 dependency lock must reject another architecture' >&2
  exit 1
fi
[ ! -s "$DEPENDENCY_CALLS" ]
echo 'Spotifyd build dependency tests passed'
