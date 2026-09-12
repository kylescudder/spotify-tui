#!/bin/sh

set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: validate-spotifyd-linux-runtime.sh SPOTIFYD_BINARY" >&2
  exit 2
fi

spotifyd_binary=$1
readelf_program=${SPOTIFY_TUI_READELF:-readelf}
max_glibc=${SPOTIFY_TUI_MAX_GLIBC:-2.34}

if ! printf '%s\n' "$max_glibc" | grep -Eq '^[0-9]+\.[0-9]+$'; then
  echo "invalid maximum glibc version: $max_glibc" >&2
  exit 2
fi

if [ ! -f "$spotifyd_binary" ]; then
  echo "missing Spotifyd binary: $spotifyd_binary" >&2
  exit 1
fi

dynamic_section=$("$readelf_program" -d "$spotifyd_binary") || {
  echo "could not inspect Spotifyd runtime dependencies: $spotifyd_binary" >&2
  exit 1
}

if printf '%s\n' "$dynamic_section" |
  grep -Eq 'lib(ssl|crypto)\.so\.1\.1([^0-9.]|$)'; then
  echo "Spotifyd requires obsolete OpenSSL 1.1 runtime libraries: $spotifyd_binary" >&2
  exit 1
fi

version_info=$("$readelf_program" --version-info "$spotifyd_binary") || {
  echo "could not inspect Spotifyd glibc requirements: $spotifyd_binary" >&2
  exit 1
}

if printf '%s\n' "$version_info" |
  awk -v maximum="$max_glibc" '
    BEGIN {
      split(maximum, limit, ".")
    }
    {
      remaining = $0
      while (match(remaining, /GLIBC_[0-9]+\.[0-9]+/)) {
        version = substr(remaining, RSTART + 6, RLENGTH - 6)
        split(version, current, ".")
        if (current[1] > limit[1] ||
            (current[1] == limit[1] && current[2] > limit[2])) {
          incompatible = 1
        }
        remaining = substr(remaining, RSTART + RLENGTH)
      }
    }
    END {
      exit incompatible ? 0 : 1
    }
  '; then
  echo "Spotifyd requires glibc newer than the supported $max_glibc baseline: $spotifyd_binary" >&2
  exit 1
fi

echo "Spotifyd Linux runtime dependencies are compatible"
