#!/bin/sh

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
validator="$repo_root/scripts/validate-release-readiness.sh"
workflow="$repo_root/.github/workflows/release.yml"
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT HUP INT TERM

fail() {
  echo "release safety test failed: $*" >&2
  exit 1
}

[ -f "$validator" ] || fail "missing release-readiness validator"

cat >"$fixture_dir/approved.toml" <<'EOF'
version = 1
public_release_version = "0.1.0"

[platforms]
linux = "0.1.0"
macos = "0.1.0"
windows = "0.1.0"
EOF

sh "$validator" 0.1.0 "$fixture_dir/approved.toml" >/dev/null ||
  fail "matching version approvals should pass"

if sh "$validator" 0.2.0 "$fixture_dir/approved.toml" >/dev/null 2>&1; then
  fail "approvals from an earlier package version must not carry forward"
fi

cat >"$fixture_dir/partial.toml" <<'EOF'
version = 1
public_release_version = "0.1.0"

[platforms]
linux = "0.1.0"
macos = ""
windows = "0.1.0"
EOF

if sh "$validator" 0.1.0 "$fixture_dir/partial.toml" >/dev/null 2>&1; then
  fail "a missing platform approval must block publication"
fi

job_block() {
  awk -v job="$1" '
    $0 == "  " job ":" { inside = 1 }
    inside && $0 == "  " job ":" { print; next }
    inside && /^  [[:alnum:]_-]+:$/ { exit }
    inside { print }
  ' "$workflow"
}

publish_job=$(job_block publish)
tap_job=$(job_block update-homebrew-tap)

printf '%s\n' "$publish_job" |
  grep -Eq '^[[:space:]]+needs:.*update-homebrew-tap' ||
  fail "GitHub publication must depend on a successful Homebrew tap update"

if printf '%s\n' "$tap_job" |
  grep -Eq '^[[:space:]]+needs:.*(^|[,[[:space:]])publish([],[:space:]]|$)'; then
  fail "the Homebrew tap update must happen before GitHub publication"
fi

echo "release safety checks passed"
