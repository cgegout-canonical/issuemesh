#!/usr/bin/env bash
set -euo pipefail

# IssueMesh Snap Packaging Script
# Assembles executable, web frontend assets, SQL datastore schema, and metadata into a strictly confined Snap package.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION=$(grep -m 1 '^version = ' Cargo.toml | cut -d '"' -f 2)
ARCH="amd64"
SNAP_NAME="issuemesh_${VERSION}_${ARCH}.snap"
STAGING_DIR="$REPO_ROOT/target/snap-staging"

echo "============================================================"
echo "Packaging IssueMesh Snap v${VERSION} (${ARCH})"
echo "============================================================"

# 1. Compile release binary
echo "==> Building optimized release binary..."
CFLAGS="-fno-sanitize=undefined -fwrapv" cargo build --release

# 2. Prepare staging tree
echo "==> Preparing Snap staging layout at $STAGING_DIR..."
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR/meta/gui" \
         "$STAGING_DIR/bin" \
         "$STAGING_DIR/usr/share/issuemesh/static" \
         "$STAGING_DIR/usr/share/issuemesh/schema"

# 3. Copy application binary and launcher
echo "==> Staging executable and launcher..."
cp target/release/issuemesh "$STAGING_DIR/bin/issuemesh"
chmod 755 "$STAGING_DIR/bin/issuemesh"

cp snap/local/issuemesh-launch "$STAGING_DIR/bin/issuemesh-launch"
chmod 755 "$STAGING_DIR/bin/issuemesh-launch"

# 4. Copy web frontend assets
echo "==> Staging web frontend assets..."
cp -r static/* "$STAGING_DIR/usr/share/issuemesh/static/"

# 5. Copy SQL datastore schema
echo "==> Staging SQL datastore schema..."
cp -r schema/* "$STAGING_DIR/usr/share/issuemesh/schema/"

# 6. Copy Desktop entry and Icon
echo "==> Staging GUI desktop entry and icon..."
cp snap/gui/issuemesh.desktop "$STAGING_DIR/meta/gui/issuemesh.desktop"
cp snap/gui/icon.svg "$STAGING_DIR/meta/gui/icon.svg"

# 7. Generate runtime meta/snap.yaml
echo "==> Generating meta/snap.yaml..."
cat << EOF > "$STAGING_DIR/meta/snap.yaml"
name: issuemesh
version: '${VERSION}'
summary: Autonomous multi-tracker issue intelligence engine
description: |
  IssueMesh is an autonomous local-first issue intelligence engine that
  federates issues, discussions, tasks, and code reviews across Launchpad,
  GitHub, and Jira with deep graph expansion and hybrid ranking.
base: core24
confinement: strict
grade: stable
architectures:
  - ${ARCH}
apps:
  issuemesh:
    command: bin/issuemesh-launch
    plugs:
      - network
      - network-bind
      - home
EOF

# 8. Validate metadata and permissions
echo "==> Validating Snap package skeleton..."
snap pack --check-skeleton "$STAGING_DIR"

# 9. Pack SquashFS Snap
echo "==> Packing ${SNAP_NAME}..."
snap pack "$STAGING_DIR" --filename="$SNAP_NAME"

echo "============================================================"
echo "Successfully created Snap package: ${SNAP_NAME}"
ls -lh "$SNAP_NAME"
echo "============================================================"
echo "To test locally under strict confinement:"
echo "  sudo snap install --dangerous ./${SNAP_NAME}"
echo "  sudo snap connect issuemesh:network"
echo "  sudo snap connect issuemesh:network-bind"
echo "  sudo snap connect issuemesh:home"
echo "  issuemesh"
echo "============================================================"
