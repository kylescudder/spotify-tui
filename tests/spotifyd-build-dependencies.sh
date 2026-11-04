#!/bin/sh

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT HUP INT TERM
mkdir "$fixture_dir/bin"

# Isolate the system source-list operations; exercise the real APT invocation.
cat >"$fixture_dir/bin/sed" <<'MOCK'
#!/bin/sh
exit 0
MOCK
cat >"$fixture_dir/bin/grep" <<'MOCK'
#!/bin/sh
exit 1
MOCK
cat >"$fixture_dir/bin/apt-get" <<'MOCK'
#!/bin/sh
printf '%s\n' "$*" >>"$APT_CALLS"
case " $* " in
  *' update '*)
    if [ "$APT_SCENARIO" = outage ]; then
      echo 'Failed to fetch snapshot index: 504 Gateway Timeout' >&2
      case " $* " in
        *' APT::Update::Error-Mode=any '*) exit 100 ;;
        *) exit 0 ;;
      esac
    fi
    ;;
  *' install '*)
    if [ "$APT_SCENARIO" = outage ]; then
      echo 'Unable to locate package libasound2-dev' >&2
      exit 100
    fi
    ;;
  *) exit 2 ;;
esac
MOCK
chmod +x "$fixture_dir/bin/"*

export APT_CALLS="$fixture_dir/apt-calls"
export APT_SCENARIO=outage
if PATH="$fixture_dir/bin:$PATH" sh "$repo_root/scripts/install-spotifyd-bullseye-build-dependencies.sh" >"$fixture_dir/output" 2>&1; then
  echo 'index download failure must fail dependency installation' >&2
  exit 1
fi
if grep -Eq '(^| )install ' "$APT_CALLS"; then
  echo 'must not install packages after snapshot index download failure' >&2
  cat "$fixture_dir/output" >&2
  exit 1
fi

export APT_SCENARIO=success
: >"$APT_CALLS"
PATH="$fixture_dir/bin:$PATH" sh "$repo_root/scripts/install-spotifyd-bullseye-build-dependencies.sh"
[ "$(wc -l <"$APT_CALLS" | tr -d ' ')" = 2 ]
grep ' update$' "$APT_CALLS" | grep -q 'Acquire::Check-Valid-Until=false'
grep ' update$' "$APT_CALLS" | grep -q 'Acquire::Retries=5'
grep -E '(^| )install ' "$APT_CALLS" | grep -q 'Acquire::Retries=5'
grep -E '(^| )install ' "$APT_CALLS" | grep -q 'libasound2-dev libdbus-1-dev libpulse-dev'
echo 'Spotifyd build dependency tests passed'
