#!/bin/sh
set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: render-homebrew-formula.sh OWNER/REPO VERSION SOURCE_SHA256 OUTPUT" >&2
  exit 2
fi

repository=$1
version=${2#v}
source_sha256=$3
output=$4
script_dir=$(CDPATH='' cd -- "$(dirname "$0")" && pwd)
template="$script_dir/../packaging/homebrew/spotify-tui.rb.template"

case "$repository" in
  ""|*/*/*|/*|*/|*[!A-Za-z0-9_.-]*/*|*/*[!A-Za-z0-9_.-]*)
    echo "repository must be OWNER/REPO" >&2
    exit 2
    ;;
  */*) ;;
  *) echo "repository must be OWNER/REPO" >&2; exit 2 ;;
esac

if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$'; then
  echo "invalid semantic version: $version" >&2
  exit 2
fi

case "$source_sha256" in
  *[!0-9a-fA-F]*)
    echo "source checksum must contain exactly 64 hexadecimal characters" >&2
    exit 2
    ;;
esac
if [ "${#source_sha256}" -ne 64 ]; then
  echo "source checksum must contain exactly 64 hexadecimal characters" >&2
  exit 2
fi

sed \
  -e "s|@REPOSITORY@|$repository|g" \
  -e "s|@VERSION@|$version|g" \
  -e "s|@SOURCE_SHA256@|$source_sha256|g" \
  "$template" > "$output"
