#!/bin/sh
set -eu

repository=${SPOTIFY_TUI_REPOSITORY:-"@REPOSITORY@"}
version=${SPOTIFY_TUI_VERSION:-}
install_dir=${SPOTIFY_TUI_INSTALL_DIR:-}
release_base=${SPOTIFY_TUI_RELEASE_BASE_URL:-}
allow_insecure=${SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS:-0}

usage() {
  cat <<'EOF'
Install Spotify TUI from a tagged GitHub release.

Usage: install.sh [OPTIONS]

Options:
  --version VERSION       Install a specific semantic version (default: latest)
  --install-dir DIRECTORY Install binaries here (default: $HOME/.local/bin)
  --repository OWNER/REPO Override the release repository
  -h, --help              Show this help

The same settings can be supplied through SPOTIFY_TUI_VERSION,
SPOTIFY_TUI_INSTALL_DIR, and SPOTIFY_TUI_REPOSITORY.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || { echo "install.sh: --version requires a value" >&2; exit 2; }
      version=$2
      shift 2
      ;;
    --install-dir)
      [ "$#" -ge 2 ] || { echo "install.sh: --install-dir requires a value" >&2; exit 2; }
      install_dir=$2
      shift 2
      ;;
    --repository)
      [ "$#" -ge 2 ] || { echo "install.sh: --repository requires a value" >&2; exit 2; }
      repository=$2
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "install.sh: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$repository" in
  @REPOSITORY@|""|*/*/*|/*|*/|*[!A-Za-z0-9_.-]*/*|*/*[!A-Za-z0-9_.-]*)
    echo "install.sh: repository must be OWNER/REPO; this source script is stamped during release" >&2
    exit 2
    ;;
  */*) ;;
  *)
    echo "install.sh: repository must be OWNER/REPO" >&2
    exit 2
    ;;
esac

if [ -n "$version" ]; then
  version=${version#v}
  if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$'; then
    echo "install.sh: invalid semantic version: $version" >&2
    exit 2
  fi
fi

if [ -z "$install_dir" ]; then
  [ -n "${HOME:-}" ] || { echo "install.sh: HOME is unset; pass --install-dir" >&2; exit 2; }
  install_dir="$HOME/.local/bin"
fi

case "$(uname -s)" in
  Linux) os=unknown-linux-musl ;;
  Darwin) os=apple-darwin ;;
  *)
    echo "install.sh: unsupported operating system: $(uname -s)" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64|amd64) architecture=x86_64 ;;
  arm64|aarch64) architecture=aarch64 ;;
  *)
    echo "install.sh: unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

target="${architecture}-${os}"
artifact="spotify-tui-${target}.tar.gz"

if [ -z "$release_base" ]; then
  if [ -n "$version" ]; then
    release_base="https://github.com/${repository}/releases/download/v${version}"
  else
    release_base="https://github.com/${repository}/releases/latest/download"
  fi
fi

case "$release_base" in
  https://*) ;;
  *)
    if [ "$allow_insecure" != 1 ]; then
      echo "install.sh: refusing a non-HTTPS release URL" >&2
      exit 1
    fi
    ;;
esac

temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/spotify-tui.XXXXXX")
trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

download() {
  name=$1
  destination=$2

  if [ -d "$release_base" ]; then
    cp "$release_base/$name" "$destination"
  else
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -LsSf \
      "$release_base/$name" -o "$destination"
  fi
}

download SHA256SUMS "$temporary_dir/SHA256SUMS"
download "$artifact" "$temporary_dir/$artifact"

expected_checksum=$(awk -v name="$artifact" '$2 == name || $2 == ("*" name) { print $1; exit }' "$temporary_dir/SHA256SUMS")
[ -n "$expected_checksum" ] || {
  echo "install.sh: $artifact is absent from SHA256SUMS" >&2
  exit 1
}

if command -v sha256sum >/dev/null 2>&1; then
  actual_checksum=$(sha256sum "$temporary_dir/$artifact" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
  actual_checksum=$(shasum -a 256 "$temporary_dir/$artifact" | awk '{ print $1 }')
else
  echo "install.sh: sha256sum or shasum is required" >&2
  exit 1
fi

if [ "$actual_checksum" != "$expected_checksum" ]; then
  echo "install.sh: checksum verification failed for $artifact" >&2
  exit 1
fi

mkdir "$temporary_dir/unpacked"
tar -xzf "$temporary_dir/$artifact" -C "$temporary_dir/unpacked"

for binary in spotify-tui spotify-tui-diagnose; do
  [ -f "$temporary_dir/unpacked/$binary" ] || {
    echo "install.sh: release archive is missing $binary" >&2
    exit 1
  }
done

mkdir -p "$install_dir"
install -m 0755 "$temporary_dir/unpacked/spotify-tui" "$install_dir/spotify-tui"
install -m 0755 "$temporary_dir/unpacked/spotify-tui-diagnose" "$install_dir/spotify-tui-diagnose"

echo "Installed spotify-tui and spotify-tui-diagnose in $install_dir"
case ":${PATH}:" in
  *:"$install_dir":*) ;;
  *) echo "Add $install_dir to PATH before running spotify-tui." ;;
esac
if ! command -v spotifyd >/dev/null 2>&1; then
  echo "spotifyd was not found on PATH; install it before authenticating or playing music."
fi
