#!/bin/sh

set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: apply-linux-portability.sh SPOTIFYD_SOURCE" >&2
  exit 1
fi

spotifyd_source=$1
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

git -C "$spotifyd_source" apply --recount --no-index \
  "$script_dir/linux-rustls.patch"
