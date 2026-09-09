#!/bin/sh

set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 CANDIDATE_VERSION FORMULA_PATH" >&2
  exit 2
fi

candidate_version=${1#v}
formula_path=$2
semver_pattern='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'

if ! printf '%s\n' "$candidate_version" | grep -Eq "$semver_pattern"; then
  echo "candidate Homebrew version is not semantic: $candidate_version" >&2
  exit 1
fi

if [ ! -e "$formula_path" ]; then
  echo "no existing Homebrew formula; accepting $candidate_version"
  exit 0
fi

current_version=$(
  sed -n '
    /releases\/download\/v/ {
      s#.*releases/download/v\([^/"[:space:]]*\)/.*#\1#
      p
      q
    }
  ' "$formula_path"
)

if ! printf '%s\n' "$current_version" | grep -Eq "$semver_pattern"; then
  echo "could not read a semantic release version from $formula_path" >&2
  exit 1
fi

normalize_semver() {
  version_without_build=${1%%+*}
  case "$version_without_build" in
    *-*)
      printf '%s~%s\n' "${version_without_build%%-*}" "${version_without_build#*-}"
      ;;
    *)
      printf '%s\n' "$version_without_build"
      ;;
  esac
}

current_order=$(normalize_semver "$current_version")
candidate_order=$(normalize_semver "$candidate_version")
oldest=$(printf '%s\n%s\n' "$current_order" "$candidate_order" | LC_ALL=C sort -V | sed -n '1p')

if [ "$oldest" != "$current_order" ]; then
  echo "refusing to downgrade Homebrew from $current_version to $candidate_version" >&2
  exit 1
fi

echo "Homebrew formula may advance from $current_version to $candidate_version"
