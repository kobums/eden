#!/bin/bash
# terminal-dev.app 번들을 만든다.
#   ./scripts/bundle.sh            # 릴리스 빌드 후 번들 생성
#   ./scripts/bundle.sh --no-build # 이미 빌드된 바이너리로 번들만 생성
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="terminal-dev"
BIN_NAME="terminal"
VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
BUNDLE="dist/${APP_NAME}.app"
CONTENTS="${BUNDLE}/Contents"

if [[ "${1:-}" != "--no-build" ]]; then
  echo "==> cargo build --release"
  cargo build --release
fi

echo "==> 번들 구성: ${BUNDLE}"
rm -rf "${BUNDLE}"
mkdir -p "${CONTENTS}/MacOS" "${CONTENTS}/Resources"
cp "target/release/${BIN_NAME}" "${CONTENTS}/MacOS/${BIN_NAME}"

cat > "${CONTENTS}/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key><string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key><string>dev.terminal.app</string>
  <key>CFBundleExecutable</key><string>${BIN_NAME}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST

# 아이콘이 있으면 넣는다 (scripts/make-icon.sh로 생성 가능)
if [[ -f "dist/AppIcon.icns" ]]; then
  cp "dist/AppIcon.icns" "${CONTENTS}/Resources/AppIcon.icns"
fi

echo "==> 완료: ${BUNDLE}"
echo "    실행: open ${BUNDLE}"
echo "    배포하려면 코드 서명 + 공증 필요 (README 참고)"
