#!/bin/sh

set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: $0 FORMULA_PATH VERSION SOURCE_URL OUTPUT" >&2
  exit 2
fi

formula_path=$1
version=${2#v}
source_url=$3
output=$4

if [ ! -f "$formula_path" ]; then
  echo "Homebrew formula does not exist: $formula_path" >&2
  exit 1
fi

if [ "$formula_path" = "$output" ]; then
  echo "staged Homebrew formula output must differ from its input" >&2
  exit 2
fi

if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$'; then
  echo "invalid semantic version: $version" >&2
  exit 2
fi

case "$source_url" in
  file://*) ;;
  *) echo "staged Homebrew source URL must use file://" >&2; exit 2 ;;
esac

awk -v source_url="$source_url" -v version="$version" '
  !staged && /^  url / {
    print "  url \"" source_url "\""
    print "  version \"" version "\""
    staged = 1
    next
  }
  { print }
  END {
    if (!staged) {
      print "Homebrew formula has no stable URL to stage" > "/dev/stderr"
      exit 1
    }
  }
' "$formula_path" > "$output"
