#!/usr/bin/env bash
# Downloads the test ROMs and CPU test vectors into test-roms/ (git-ignored).
#
# Nothing fetched here is redistributed with Glassbit. Works on Linux, macOS
# and Git Bash on Windows. Safe to re-run: steps already done are skipped.
#
# Usage: scripts/fetch-test-roms.sh [--no-sm83]
set -euo pipefail

# Prebuilt Blargg, Mooneye, dmg-acid2 and other suites, pinned by version.
ROMS_VERSION="v7.0"
ROMS_URL="https://github.com/c-sp/game-boy-test-roms/releases/download/${ROMS_VERSION}/game-boy-test-roms-${ROMS_VERSION}.zip"
ROMS_SHA256="b9a9d7a1075aa35a3d07c07c34974048672d8520dca9e07a50178f5860c3832c"

# Per-instruction JSON tests for the SM83 CPU, pinned by commit.
SM83_REPO="https://github.com/SingleStepTests/sm83.git"
SM83_COMMIT="f9c30210245dd691661db39f5ace022c465ecc2f"

fetch_sm83=1
for arg in "$@"; do
  case "$arg" in
    --no-sm83) fetch_sm83=0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/test-roms"
mkdir -p "$dest"

roms_dir="$dest/game-boy-test-roms"
if [ -f "$roms_dir/.version" ] && [ "$(cat "$roms_dir/.version")" = "$ROMS_VERSION" ]; then
  echo "game-boy-test-roms $ROMS_VERSION already present"
else
  echo "downloading game-boy-test-roms $ROMS_VERSION"
  zip="$dest/game-boy-test-roms.zip"
  curl -fsSL -o "$zip" "$ROMS_URL"
  actual="$(sha256 "$zip")"
  if [ "$actual" != "$ROMS_SHA256" ]; then
    echo "checksum mismatch for $zip" >&2
    echo "  expected $ROMS_SHA256" >&2
    echo "  actual   $actual" >&2
    exit 1
  fi
  rm -rf "$roms_dir"
  mkdir -p "$roms_dir"
  unzip -q "$zip" -d "$roms_dir"
  rm "$zip"
  echo "$ROMS_VERSION" > "$roms_dir/.version"
fi

if [ "$fetch_sm83" = 1 ]; then
  sm83_dir="$dest/sm83"
  if [ -d "$sm83_dir/.git" ] && [ "$(git -C "$sm83_dir" rev-parse HEAD)" = "$SM83_COMMIT" ]; then
    echo "sm83 tests already present"
  else
    echo "fetching sm83 tests (large, this can take a while)"
    rm -rf "$sm83_dir"
    git init -q "$sm83_dir"
    git -C "$sm83_dir" remote add origin "$SM83_REPO"
    git -C "$sm83_dir" fetch -q --depth 1 origin "$SM83_COMMIT"
    git -C "$sm83_dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
  fi
fi

echo "test ROMs ready in $dest"
