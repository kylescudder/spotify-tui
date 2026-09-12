#!/bin/sh

set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
validator="$repo_root/scripts/validate-release-readiness.sh"
formula_version_guard="$repo_root/scripts/validate-homebrew-formula-upgrade.sh"
formula_validator="$repo_root/scripts/validate-homebrew-formula.sh"
spotifyd_build_dependencies="$repo_root/scripts/install-spotifyd-bullseye-build-dependencies.sh"
workflow="$repo_root/.github/workflows/release.yml"
ci_workflow="$repo_root/.github/workflows/ci.yml"
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT HUP INT TERM

fail() {
  echo "release safety test failed: $*" >&2
  exit 1
}

[ -f "$validator" ] || fail "missing release-readiness validator"
[ -f "$formula_version_guard" ] || fail "missing Homebrew formula version guard"
[ -f "$formula_validator" ] || fail "missing staged Homebrew formula validator"
[ -f "$spotifyd_build_dependencies" ] || fail "missing pinned Spotifyd build dependencies"

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
  source_file=${2:-$workflow}
  awk -v job="$1" '
    $0 == "  " job ":" { inside = 1 }
    inside && $0 == "  " job ":" { print; next }
    inside && /^  [[:alnum:]_-]+:$/ { exit }
    inside { print }
  ' "$source_file"
}

step_block() {
  awk -v step="$1" '
    $0 == "      - name: " step { inside = 1 }
    inside && $0 == "      - name: " step { print; next }
    inside && /^      - name:/ { exit }
    inside { print }
  ' "$workflow"
}

matrix_entry() {
  awk -v os="$1" '
    $0 == "          - os: " os { inside = 1 }
    inside && $0 != "          - os: " os && /^          - os:/ { exit }
    inside { print }
  ' "$workflow"
}

publish_job=$(job_block publish)
tap_job=$(job_block update-homebrew-tap)
tap_access_job=$(job_block test-homebrew-tap-access)
binaries_job=$(job_block binaries)
spotifyd_build_step=$(step_block "Build pinned Spotifyd runtime")
spotifyd_patch_step=$(step_block "Add Spotify TUI local control to Spotifyd")
linux_runtime_step=$(step_block "Validate Linux Spotifyd runtime dependencies")
homebrew_style_step=$(step_block "Validate formula style")
arm_linux_release=$(matrix_entry ubuntu-24.04-arm)
x86_linux_release=$(matrix_entry ubuntu-24.04)
arm_runtime_job=$(job_block spotifyd-linux-runtime "$ci_workflow")
release_arm_runtime_job=$(job_block spotifyd-linux-arm64)

printf '%s\n' "$arm_linux_release" |
  grep -Fq 'spotifyd_mode: artifact' ||
  fail "Linux ARM64 releases must consume the portable runtime artifact"

printf '%s\n' "$arm_linux_release" |
  grep -Fq 'spotifyd_max_glibc: "2.31"' ||
  fail "Linux ARM64 packages must enforce their glibc 2.31 baseline"

printf '%s\n' "$x86_linux_release" |
  grep -Fq 'spotifyd_max_glibc: "2.34"' ||
  fail "Linux x86_64 packages must enforce their published glibc baseline"

printf '%s\n' "$binaries_job" |
  grep -Fq 'needs: [preflight, spotifyd-linux-arm64]' ||
  fail "release packages must wait for the portable ARM64 runtime artifact"

printf '%s\n' "$release_arm_runtime_job" |
  grep -Fq 'runs-on: ubuntu-24.04-arm' ||
  fail "release CI must build Spotifyd natively on ARM64"

for runtime_job in "$release_arm_runtime_job" "$arm_runtime_job"; do
  printf '%s\n' "$runtime_job" |
    grep -Fq 'container: rust:1.88.0-bullseye@sha256:' ||
    fail "Linux ARM64 Spotifyd must use the pinned glibc 2.31 build container"

  printf '%s\n' "$runtime_job" |
    grep -Fq 'sh packaging/spotifyd/apply-linux-portability.sh spotifyd-upstream' ||
    fail "Linux ARM64 Spotifyd must replace native TLS with Rustls"

  printf '%s\n' "$runtime_job" |
    grep -Fq 'sh scripts/install-spotifyd-bullseye-build-dependencies.sh' ||
    fail "Linux ARM64 Spotifyd must use the pinned Debian package snapshot"

  printf '%s\n' "$runtime_job" |
    grep -Fq -- '--features alsa_backend,pulseaudio_backend,dbus_mpris' ||
    fail "Linux ARM64 Spotifyd must retain its playback and MPRIS backends"

  printf '%s\n' "$runtime_job" |
    grep -Fq 'sh scripts/validate-spotifyd-linux-runtime.sh' ||
    fail "Linux ARM64 Spotifyd builds must validate runtime dependencies"

  printf '%s\n' "$runtime_job" |
    grep -Fq 'SPOTIFY_TUI_MAX_GLIBC: "2.31"' ||
    fail "Linux ARM64 Spotifyd builds must enforce their glibc 2.31 baseline"

  if printf '%s\n' "$runtime_job" | grep -Fq 'libssl-dev'; then
    fail "the Rustls ARM64 runtime must not retain a system OpenSSL build dependency"
  fi
done

grep -Fq 'snapshot_timestamp=20250724T000000Z' "$spotifyd_build_dependencies" ||
  fail "Spotifyd build dependencies must come from an immutable Debian snapshot"

printf '%s\n' "$spotifyd_patch_step" |
  grep -Fq "if: matrix.spotifyd_patch == true" ||
  fail "the local-control patch must only be applied to its configured release targets"

# The GitHub expression is intentionally matched literally.
# shellcheck disable=SC2016
printf '%s\n' "$spotifyd_build_step" |
  grep -Fq -- '--features ${{ matrix.spotifyd_features }}' ||
  fail "source-built Spotifyd releases must use their platform feature set"

# The shell variable is intentionally matched literally in the workflow source.
# shellcheck disable=SC2016
printf '%s\n' "$linux_runtime_step" |
  grep -Fq 'sh scripts/validate-spotifyd-linux-runtime.sh "$spotifyd_binary"' ||
  fail "Linux release binaries must reject obsolete runtime dependencies"

printf '%s\n' "$arm_runtime_job" |
  grep -Fq 'runs-on: ubuntu-24.04-arm' ||
  fail "normal CI must build the Linux ARM64 Spotifyd runtime natively"

printf '%s\n' "$spotifyd_build_step" |
  grep -Eq '^[[:space:]]+shell:[[:space:]]+bash$' ||
  fail "the cross-platform Spotifyd build must run its Bash continuations under Bash"

printf '%s\n' "$homebrew_style_step" |
  grep -Fq 'sh scripts/validate-homebrew-formula.sh bundle/spotify-tui.rb' ||
  fail "release rehearsal must validate the formula through the staged-tap helper"

grep -Fq "brew tap-new --no-git \"\$tap_name\"" "$formula_validator" ||
  fail "Homebrew validation must stage the formula in a disposable tap"

grep -Fq "brew style --formula \"\$formula_name\"" "$formula_validator" ||
  fail "Homebrew style must validate the staged tap formula"

grep -Fq "brew audit --strict --formula \"\$formula_name\"" "$formula_validator" ||
  fail "Homebrew audit must validate the staged tap formula"

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
