#!/usr/bin/env bash
set -euo pipefail

# Usage: ./release.sh patch|minor|major
# Bumps the version, tags, pushes, and publishes both crates.

BUMP="${1:-patch}"
if [[ "$BUMP" != "patch" && "$BUMP" != "minor" && "$BUMP" != "major" ]]; then
    echo "Usage: $0 patch|minor|major" >&2
    exit 1
fi

# Read current version from Cargo.toml
CURRENT=$(grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$BUMP" in
    patch) PATCH=$((PATCH + 1)) ;;
    minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
    major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
esac

NEW="$MAJOR.$MINOR.$PATCH"
TAG="v$NEW"
TODAY=$(date +%Y-%m-%d)

echo "Bumping $CURRENT → $NEW"

# Bump versions in both Cargo.toml files
sed -i '' "s/^version = \"$CURRENT\"/version = \"$NEW\"/" Cargo.toml
sed -i '' "s/^version = \"$CURRENT\"/version = \"$NEW\"/" gaman-macros/Cargo.toml
# Update the inter-crate dependency version
sed -i '' "s/gaman-macros = { version = \"$CURRENT\"/gaman-macros = { version = \"$NEW\"/" Cargo.toml

# Move [Unreleased] section in CHANGELOG to the new version
# Replaces the first occurrence of "## [Unreleased]" with both a new Unreleased and the new version heading
CHANGELOG_ENTRY="## [Unreleased]\n\n## [$NEW] - $TODAY"
# Use perl for reliable in-place multiline sed on macOS
perl -i -0pe "s/## \[Unreleased\]/$CHANGELOG_ENTRY/" CHANGELOG.md

# Update the comparison links at the bottom of CHANGELOG.md
# Insert new version link after [unreleased] link
PREV="$CURRENT"
perl -i -pe "
    s|\[unreleased\]: (.*)/compare/v${PREV}\.\.\.HEAD|[unreleased]: \$1/compare/${TAG}...HEAD\n[${NEW}]: \$1/compare/v${PREV}...${TAG}|i
" CHANGELOG.md

cargo build --quiet

git add -u
git add Cargo.toml gaman-macros/Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore: release $TAG"
git tag "$TAG"
git push
git push origin "$TAG"

cargo publish -p gaman-macros
cargo publish -p gaman

echo "Released $TAG"
