#!/usr/bin/env bash
# scripts/release.sh — Bump versions in all Cargo.toml files, commit, and tag.
#
# Usage:
#   ./scripts/release.sh patch     # 0.1.0 → 0.1.1
#   ./scripts/release.sh minor     # 0.1.0 → 0.2.0
#   ./scripts/release.sh major     # 0.1.0 → 1.0.0
#   ./scripts/release.sh 0.3.0     # explicit version
#
set -euo pipefail

BUMP=${1:-}
if [ -z "$BUMP" ]; then
    echo "Usage: $0 <patch|minor|major|x.y.z>"
    exit 1
fi

# ── locate repo root ──────────────────────────────────────────────────────────
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# ── read current version from workspace root Cargo.toml ──────────────────────
CURRENT=$(grep -m1 '^version = ' crates/cli/Cargo.toml | sed 's/version = "\(.*\)"/\1/')
echo "Current version: $CURRENT"

# ── compute next version ──────────────────────────────────────────────────────
bump_version() {
    local ver="$1" part="$2"
    local major minor patch
    IFS='.' read -r major minor patch <<< "$ver"
    case "$part" in
        major) echo "$((major + 1)).0.0" ;;
        minor) echo "${major}.$((minor + 1)).0" ;;
        patch) echo "${major}.${minor}.$((patch + 1))" ;;
        *)     echo "$part" ;;  # explicit version string
    esac
}

NEXT=$(bump_version "$CURRENT" "$BUMP")
echo "Next version:    $NEXT"

# Sanity check: must look like x.y.z
if ! echo "$NEXT" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "error: '$NEXT' is not a valid semver (expected x.y.z)"
    exit 1
fi

# Guard against downgrade
if [ "$NEXT" = "$CURRENT" ]; then
    echo "error: new version equals current version ($CURRENT)"
    exit 1
fi

read -r -p "Bump $CURRENT → $NEXT and create tag v$NEXT? [y/N] " confirm
case "$confirm" in y|Y) ;; *) echo "Aborted."; exit 0 ;; esac

# ── patch version in all crates ───────────────────────────────────────────────
CRATES=(compiler server cli registry)
for crate in "${CRATES[@]}"; do
    manifest="crates/$crate/Cargo.toml"
    if [ -f "$manifest" ]; then
        sed -i.bak "s/^version = \"$CURRENT\"/version = \"$NEXT\"/" "$manifest"
        rm -f "${manifest}.bak"
        echo "  Patched $manifest"
    fi
done

# ── update Cargo.lock ─────────────────────────────────────────────────────────
cargo update -p nexa -p nexa-compiler -p nexa-server -p nexa-registry \
    2>/dev/null || cargo generate-lockfile
echo "  Updated Cargo.lock"

# ── commit ────────────────────────────────────────────────────────────────────
git add crates/*/Cargo.toml Cargo.lock
git commit -m "chore: bump version to $NEXT"
echo "  Committed version bump"

# ── tag ───────────────────────────────────────────────────────────────────────
git tag -a "v$NEXT" -m "Release v$NEXT"
echo "  Created tag v$NEXT"

echo ""
echo "  Done! Push to trigger the release CI:"
echo "  git push && git push origin v$NEXT"
