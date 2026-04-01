#!/bin/bash
set -euo pipefail

# Build a macOS .app bundle and .dmg for cull.
# Usage: ./scripts/bundle.sh
# Output: dist/Cull.app and dist/Cull-<version>.dmg

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
BUNDLE_ID="com.getcull.cull"
APP_NAME="Cull"
DMG_NAME="Cull-${VERSION}"

echo "==> Building cull v${VERSION} release binary..."
cargo build --release

echo "==> Creating .app bundle..."
rm -rf dist
mkdir -p "dist/${APP_NAME}.app/Contents/MacOS"
mkdir -p "dist/${APP_NAME}.app/Contents/Resources"

cp target/release/cull "dist/${APP_NAME}.app/Contents/MacOS/cull"
cp assets/icon.icns "dist/${APP_NAME}.app/Contents/Resources/AppIcon.icns"

cat > "dist/${APP_NAME}.app/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>cull</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Folder</string>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.folder</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
PLIST

echo "    Created dist/${APP_NAME}.app"

echo "==> Creating DMG..."
DMG_TMP="dist/${DMG_NAME}-tmp.dmg"
DMG_FINAL="dist/${DMG_NAME}.dmg"
MOUNT_DIR="/Volumes/${APP_NAME}"

# Clean up any stale mount
hdiutil detach "${MOUNT_DIR}" 2>/dev/null || true
rm -f "${DMG_TMP}" "${DMG_FINAL}"

hdiutil create -size 20m -fs HFS+ -volname "${APP_NAME}" "${DMG_TMP}"
hdiutil attach "${DMG_TMP}"

cp -R "dist/${APP_NAME}.app" "${MOUNT_DIR}/"
ln -s /Applications "${MOUNT_DIR}/Applications"

hdiutil detach "${MOUNT_DIR}"
hdiutil convert "${DMG_TMP}" -format UDZO -o "${DMG_FINAL}"
rm "${DMG_TMP}"

echo ""
echo "==> Done."
ls -lh "${DMG_FINAL}"
