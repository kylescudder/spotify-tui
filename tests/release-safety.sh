#!/bin/sh

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
validator="$repo_root/scripts/validate-release-readiness.sh"
formula_version_guard="$repo_root/scripts/validate-homebrew-formula-upgrade.sh"
workflow="$repo_root/.github/workflows/release.yml"
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT HUP INT TERM

fail() {
  echo "release safety test failed: $*" >&2
  exit 1
}

[ -f "$validator" ] || fail "missing release-readiness validator"
[ -f "$formula_version_guard" ] || fail "missing Homebrew formula version guard"

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

step_block() {
  awk -v step="$1" '
    $0 == "      - name: " step { inside = 1 }
    inside && $0 == "      - name: " step { print; next }
    inside && /^      - name:/ { exit }
    inside { print }
  ' "$workflow"
}

publish_job=$(job_block publish)
tap_job=$(job_block update-homebrew-tap)
tap_access_job=$(job_block test-homebrew-tap-access)
spotifyd_build_step=$(step_block "Build pinned Spotifyd runtime")

printf '%s\n' "$spotifyd_build_step" |
  grep -Eq '^[[:space:]]+shell:[[:space:]]+bash$' ||
  fail "the cross-platform Spotifyd build must run its Bash continuations under Bash"

printf '%s\n' "$publish_job" |
  grep -Eq '^[[:space:]]+needs:.*update-homebrew-tap' &&
  fail "release assets must be public before the tap formula is exposed"

printf '%s\n' "$tap_job" |
  grep -Eq '^[[:space:]]+needs:.*(^|[,[[:space:]])publish([],[:space:]]|$)' ||
  fail "the Homebrew tap update must wait for public release assets"

printf '%s\n' "$tap_job" |
  grep -Fq "ssh-key: \${{ secrets.HOMEBREW_TAP_SSH_KEY }}" ||
  fail "the Homebrew tap update must use the scoped SSH deploy key"

printf '%s\n' "$tap_job" |
  grep -Fq 'git -C tap push origin HEAD:main' ||
  fail "the accepted formula must be pushed to the tap default branch"

printf '%s\n' "$tap_job" |
  grep -Fq "scripts/validate-homebrew-formula-upgrade.sh \"\$VERSION\"" ||
  fail "the tap update must reject a formula downgrade"

printf '%s\n' "$tap_access_job" |
  grep -Fq 'git -C tap commit --allow-empty' ||
  fail "the opt-in deploy-key check must create a content-neutral real commit"

printf '%s\n' "$tap_access_job" |
  grep -Fq 'git -C tap push origin HEAD:main' ||
  fail "the opt-in deploy-key check must exercise the production main-branch push"

if printf '%s\n' "$tap_access_job" | grep -Fq -- '--dry-run'; then
  fail "a dry-run push does not validate server-side write policy"
fi

grep -Fq 'verify_tap_write:' "$workflow" ||
  fail "real tap writes must require an explicit workflow-dispatch choice"

grep -Fq "group: release-\${{ github.repository }}" "$workflow" ||
  fail "release runs for different tags must be serialized"

cat >"$fixture_dir/current-formula.rb" <<'EOF'
class SpotifyTui < Formula
  url "https://github.com/kylescudder/spotify-tui/releases/download/v0.2.0/spotify-tui-source.tar.gz"
end
EOF

if sh "$formula_version_guard" 0.1.0 "$fixture_dir/current-formula.rb" >/dev/null 2>&1; then
  fail "an old tag rerun must not downgrade the tap formula"
fi

sh "$formula_version_guard" 0.2.0 "$fixture_dir/current-formula.rb" >/dev/null ||
  fail "an idempotent same-version tap update should pass"

sh "$formula_version_guard" 0.3.0 "$fixture_dir/current-formula.rb" >/dev/null ||
  fail "a newer formula version should pass"

cat >"$fixture_dir/prerelease-formula.rb" <<'EOF'
class SpotifyTui < Formula
  url "https://github.com/kylescudder/spotify-tui/releases/download/v1.0.0-alpha.2/spotify-tui-source.tar.gz"
end
EOF

sh "$formula_version_guard" 1.0.0-alpha.10 "$fixture_dir/prerelease-formula.rb" >/dev/null ||
  fail "semantic prerelease ordering should accept alpha.10 after alpha.2"

sh "$formula_version_guard" 1.0.0 "$fixture_dir/prerelease-formula.rb" >/dev/null ||
  fail "a stable version should advance a prerelease"

if sh "$formula_version_guard" 0.2.0-alpha.1 "$fixture_dir/current-formula.rb" >/dev/null 2>&1; then
  fail "a prerelease must not replace an existing stable version"
fi

sh "$formula_version_guard" 0.1.0 "$fixture_dir/missing-formula.rb" >/dev/null ||
  fail "the first tap formula should pass without an existing version"

if grep -Fq 'HOMEBREW_TAP_TOKEN' "$workflow"; then
  fail "the obsolete cross-repository PAT must not remain in the release workflow"
fi

echo "release safety checks passed"
