#!/bin/sh

set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 VERSION [RELEASE_READINESS_FILE]" >&2
  exit 2
fi

expected_version=$1
readiness_file=${2:-release-readiness.toml}

if [ ! -r "$readiness_file" ]; then
  echo "release readiness file is not readable: $readiness_file" >&2
  exit 1
fi

public_release_version=$(
  awk -F '"' '
    /^[[:space:]]*\[/ { exit }
    /^[[:space:]]*public_release_version[[:space:]]*=/ { print $2; exit }
  ' "$readiness_file"
)

if [ "$public_release_version" != "$expected_version" ]; then
  echo "public release $expected_version is not approved in $readiness_file" >&2
  echo "complete the rehearsal and live acceptance checks before approving that exact version" >&2
  exit 1
fi

for platform in linux macos windows; do
  approved_version=$(
    awk -F '"' -v platform="$platform" '
      /^[[:space:]]*\[platforms\][[:space:]]*$/ { in_platforms = 1; next }
      in_platforms && /^[[:space:]]*\[/ { exit }
      in_platforms {
        key = $1
        gsub(/[[:space:]]/, "", key)
        if (key == platform "=") {
          print $2
          exit
        }
      }
    ' "$readiness_file"
  )

  if [ "$approved_version" != "$expected_version" ]; then
    echo "$platform runtime acceptance is not approved for $expected_version" >&2
    exit 1
  fi
done

echo "release $expected_version is approved for publication"
