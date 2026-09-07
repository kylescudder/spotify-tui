#!/bin/sh
set -eu

repository=${SPOTIFY_TUI_REPOSITORY:-"@REPOSITORY@"}
version=${SPOTIFY_TUI_VERSION:-}
install_dir=${SPOTIFY_TUI_INSTALL_DIR:-}
config_dir=${SPOTIFY_TUI_SPOTIFYD_CONFIG_DIR:-}
release_base=${SPOTIFY_TUI_RELEASE_BASE_URL:-}
allow_insecure=${SPOTIFY_TUI_ALLOW_INSECURE_FOR_TESTS:-0}
install_dependencies=1
force_dependencies=0
configure_service=1

usage() {
  cat <<'EOF'
Install Spotify TUI from a tagged GitHub release.

Usage: install.sh [OPTIONS]

Options:
  --version VERSION       Install a specific semantic version (default: latest)
  --install-dir DIRECTORY Install binaries here (default: $HOME/.local/bin)
  --config-dir DIRECTORY  Write a new Spotifyd config here when needed
  --repository OWNER/REPO Override the release repository
  --no-dependencies       Install Spotify TUI without bundled Spotifyd
  --force-dependencies    Replace an existing Spotifyd with the bundled version
  --no-service            Do not create user-level Spotifyd startup integration
  -h, --help              Show this help

The same settings can be supplied through SPOTIFY_TUI_VERSION,
SPOTIFY_TUI_INSTALL_DIR, SPOTIFY_TUI_SPOTIFYD_CONFIG_DIR, and
SPOTIFY_TUI_REPOSITORY.
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
    --config-dir)
      [ "$#" -ge 2 ] || { echo "install.sh: --config-dir requires a value" >&2; exit 2; }
      config_dir=$2
      shift 2
      ;;
    --repository)
      [ "$#" -ge 2 ] || { echo "install.sh: --repository requires a value" >&2; exit 2; }
      repository=$2
      shift 2
      ;;
    --no-dependencies)
      install_dependencies=0
      shift
      ;;
    --force-dependencies)
      force_dependencies=1
      shift
      ;;
    --no-service)
      configure_service=0
      shift
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

if [ "$install_dependencies" -eq 0 ] && [ "$force_dependencies" -eq 1 ]; then
  echo "install.sh: --no-dependencies and --force-dependencies cannot be used together" >&2
  exit 2
fi

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
case "$install_dir" in
  /*) ;;
  *) install_dir="$(pwd)/$install_dir" ;;
esac

case "$(uname -s)" in
  Linux)
    os=unknown-linux-musl
    platform=linux
    ;;
  Darwin)
    os=apple-darwin
    platform=macos
    ;;
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

if [ "$install_dependencies" -eq 1 ] && [ -z "$config_dir" ]; then
  [ -n "${HOME:-}" ] || {
    echo "install.sh: HOME is unset; pass --config-dir or --no-dependencies" >&2
    exit 2
  }
  if [ "$platform" = linux ]; then
    config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/spotifyd"
  else
    config_dir="$HOME/Library/Application Support/spotifyd"
  fi
fi
if [ "$install_dependencies" -eq 1 ]; then
  case "$config_dir" in
    /*) ;;
    *) config_dir="$(pwd)/$config_dir" ;;
  esac
fi

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

spotifyd_path=
install_bundled_spotifyd=0
if [ "$install_dependencies" -eq 1 ]; then
  if [ "$force_dependencies" -eq 0 ] && [ -x "$install_dir/spotifyd" ]; then
    spotifyd_path="$install_dir/spotifyd"
  elif [ "$force_dependencies" -eq 0 ] && command -v spotifyd >/dev/null 2>&1; then
    spotifyd_path=$(command -v spotifyd)
  else
    [ -f "$temporary_dir/unpacked/spotifyd" ] || {
      echo "install.sh: release archive is missing bundled spotifyd" >&2
      exit 1
    }
    [ -f "$temporary_dir/unpacked/SPOTIFYD-LICENSE" ] || {
      echo "install.sh: release archive is missing the Spotifyd licence" >&2
      exit 1
    }
    spotifyd_path="$install_dir/spotifyd"
    install_bundled_spotifyd=1
  fi
fi

mkdir -p "$install_dir"
install -m 0755 "$temporary_dir/unpacked/spotify-tui" "$install_dir/spotify-tui"
install -m 0755 "$temporary_dir/unpacked/spotify-tui-diagnose" "$install_dir/spotify-tui-diagnose"

if [ "$install_bundled_spotifyd" -eq 1 ]; then
  install -m 0755 "$temporary_dir/unpacked/spotifyd" "$spotifyd_path"
  share_dir=$(dirname "$install_dir")/share/spotify-tui
  mkdir -p "$share_dir"
  install -m 0644 "$temporary_dir/unpacked/SPOTIFYD-LICENSE" "$share_dir/SPOTIFYD-LICENSE"
  echo "Installed the bundled Spotifyd runtime in $install_dir"
elif [ "$install_dependencies" -eq 1 ]; then
  echo "Preserved existing Spotifyd at $spotifyd_path"
fi

if [ "$install_dependencies" -eq 1 ]; then
  config_path="$config_dir/spotifyd.conf"
  if [ ! -e "$config_path" ]; then
    mkdir -p "$config_dir"
    if [ "$platform" = linux ]; then
      umask 077
      cat > "$config_path" <<'EOF'
[global]
use_mpris = true
dbus_type = "session"
volume_controller = "softvol"
initial_volume = 90
EOF
    else
      umask 077
      cat > "$config_path" <<'EOF'
[global]
volume_controller = "softvol"
initial_volume = 90
EOF
    fi
    echo "Created Spotifyd configuration at $config_path"
  else
    echo "Preserved existing Spotifyd configuration at $config_path"
  fi
fi

escape_systemd_path() {
  printf '%s' "$1" | sed -e 's/%/%%/g' -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

escape_xml() {
  printf '%s' "$1" | sed \
    -e 's/&/\&amp;/g' \
    -e 's/</\&lt;/g' \
    -e 's/>/\&gt;/g' \
    -e 's/"/\&quot;/g' \
    -e "s/'/\&apos;/g"
}

if [ "$install_dependencies" -eq 1 ] && [ "$configure_service" -eq 1 ]; then
  if [ "$platform" = linux ]; then
    systemctl_program=${SPOTIFY_TUI_SYSTEMCTL:-systemctl}
    systemd_user_dir=${SPOTIFY_TUI_SYSTEMD_USER_DIR:-"${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"}
    service_path="$systemd_user_dir/spotifyd.service"
    service_exists=0
    if command -v "$systemctl_program" >/dev/null 2>&1 \
      && "$systemctl_program" --user cat spotifyd.service >/dev/null 2>&1; then
      service_exists=1
    fi
    if [ "$service_exists" -eq 0 ] && [ ! -e "$service_path" ]; then
      escaped_spotifyd_path=$(escape_systemd_path "$spotifyd_path")
      escaped_config_path=$(escape_systemd_path "$config_path")
      mkdir -p "$systemd_user_dir"
      cat > "$service_path" <<EOF
[Unit]
Description=Spotifyd for Spotify TUI
Wants=network-online.target
After=network-online.target sound.target

[Service]
ExecStart="$escaped_spotifyd_path" --config-path "$escaped_config_path" --no-daemon
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF
      echo "Created Spotifyd user service at $service_path"
    fi
    if command -v "$systemctl_program" >/dev/null 2>&1; then
      if "$systemctl_program" --user daemon-reload >/dev/null 2>&1 \
        && "$systemctl_program" --user enable --now spotifyd.service >/dev/null 2>&1; then
        echo "Enabled and started spotifyd.service."
      else
        echo "Could not start spotifyd.service during installation; Spotify TUI will retry automatically when it launches."
      fi
    else
      echo "systemctl was not found; Spotify TUI will start Spotifyd automatically when it launches."
    fi
  else
    launchctl_program=${SPOTIFY_TUI_LAUNCHCTL:-launchctl}
    launch_agents_dir=${SPOTIFY_TUI_LAUNCH_AGENTS_DIR:-"$HOME/Library/LaunchAgents"}
    service_path="$launch_agents_dir/io.github.kylescudder.spotifyd.plist"
    service_label=io.github.kylescudder.spotifyd
    if [ ! -e "$service_path" ]; then
      escaped_spotifyd_path=$(escape_xml "$spotifyd_path")
      escaped_config_path=$(escape_xml "$config_path")
      mkdir -p "$launch_agents_dir"
      cat > "$service_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$service_label</string>
  <key>ProgramArguments</key>
  <array>
    <string>$escaped_spotifyd_path</string>
    <string>--config-path</string>
    <string>$escaped_config_path</string>
    <string>--no-daemon</string>
  </array>
  <key>KeepAlive</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
EOF
      echo "Created Spotifyd launch agent at $service_path"
    fi
    if command -v "$launchctl_program" >/dev/null 2>&1; then
      launch_domain="gui/$(id -u)"
      if "$launchctl_program" print "$launch_domain/$service_label" >/dev/null 2>&1; then
        "$launchctl_program" kickstart -k "$launch_domain/$service_label" >/dev/null 2>&1 \
          || echo "Could not restart the Spotifyd launch agent; Spotify TUI will retry automatically when it launches."
      else
        "$launchctl_program" bootstrap "$launch_domain" "$service_path" >/dev/null 2>&1 \
          || echo "Could not start the Spotifyd launch agent; Spotify TUI will retry automatically when it launches."
      fi
    else
      echo "launchctl was not found; Spotify TUI will start Spotifyd automatically when it launches."
    fi
  fi
fi

echo "Installed spotify-tui and spotify-tui-diagnose in $install_dir"
case ":${PATH}:" in
  *:"$install_dir":*) ;;
  *) echo "Add $install_dir to PATH before running spotify-tui." ;;
esac
if [ "$install_dependencies" -eq 0 ]; then
  echo "Dependency installation was skipped; ensure a compatible spotifyd is on PATH."
fi
