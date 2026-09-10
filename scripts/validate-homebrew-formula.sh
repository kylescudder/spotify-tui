#!/bin/sh

set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 FORMULA_PATH" >&2
  exit 2
fi

formula_path=$1
tap_name=spotify-tui/rehearsal
formula_name="$tap_name/spotify-tui"

if [ ! -f "$formula_path" ]; then
  echo "Homebrew formula does not exist: $formula_path" >&2
  exit 1
fi

brew tap-new --no-git "$tap_name"
tap_dir=$(brew --repository "$tap_name")
cp "$formula_path" "$tap_dir/Formula/spotify-tui.rb"

brew style --formula "$formula_name"
brew audit --strict --formula "$formula_name"
