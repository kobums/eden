#!/bin/bash
# 배포용 릴리스: Developer ID 서명 → 공증(notarize) → staple → zip → sha256.
# (Spot 의 scripts/release.sh 플로우를 이식)
#
#   ./scripts/release.sh              # dist/eden-<version>.zip 생성 + sha256 출력
#   ./scripts/release.sh --publish    # 위 + git 태그 + GitHub 릴리스 + Homebrew cask 갱신
#
# 사전 준비:
#   1) Developer ID Application 인증서 (keychain)
#   2) 공증 자격증명 — 아래 둘 중 하나:
#      · keychain 프로파일 (기본 spot-notary, NOTARY_PROFILE 로 변경 가능)
#      · 환경 변수:  NOTARY_KEY(.p8 경로) + NOTARY_KEY_ID + NOTARY_ISSUER  (CI용)
#
# --publish 옵션:
#   TAP_DIR   Homebrew tap 저장소(kobums/homebrew-tap) 로컬 체크아웃 경로.
#             지정 시 cask 를 갱신·커밋·푸시한다. gh CLI 필요.
set -euo pipefail
cd "$(dirname "$0")/.."

PUBLISH=0
[ "${1:-}" = "--publish" ] && PUBLISH=1

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
APP=dist/eden.app
DIST=dist
ZIP="$DIST/eden-$VERSION.zip"
REPO="kobums/eden"

# --- 1. Developer ID 인증서 확인 ---
IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null \
    | grep "Developer ID Application" | head -1 | awk '{print $2}')
if [ -z "$IDENTITY" ]; then
    echo "❌ Developer ID Application 인증서가 없습니다." >&2
    exit 1
fi
echo "▶ Developer ID: $IDENTITY"

# --- 2. 빌드 + 번들 ---
[ -f dist/AppIcon.icns ] || ./scripts/make-icon.sh
./scripts/bundle.sh

# --- 3. Developer ID 서명 (hardened runtime + timestamp) ---
codesign --force --options runtime --timestamp \
    --sign "$IDENTITY" "$APP"
codesign --verify --strict "$APP"
echo "▶ 서명 완료"

# --- 4. 공증용 zip (ditto 로 서명·번들 구조 보존) ---
mkdir -p "$DIST"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

# --- 5. 공증 제출 (완료까지 대기) ---
echo "▶ 공증 제출 중… (수 분 소요)"
if [ -n "${NOTARY_KEY:-}" ]; then
    xcrun notarytool submit "$ZIP" \
        --key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER" \
        --wait
else
    xcrun notarytool submit "$ZIP" \
        --keychain-profile "${NOTARY_PROFILE:-spot-notary}" \
        --wait
fi

# --- 6. 티켓 staple + 최종 재압축 ---
xcrun stapler staple "$APP"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

# --- 7. sha256 ---
SHA=$(shasum -a 256 "$ZIP" | awk '{print $1}')
echo "$SHA" > "$ZIP.sha256"

echo ""
echo "✅ 완료: $ZIP"
echo "   버전:  $VERSION"
echo "   sha256: $SHA"
echo "   검증:  spctl -a -vvv -t install $APP"

[ "$PUBLISH" -eq 0 ] && exit 0

# --- 8. 게시: git 태그 + GitHub 릴리스 ---
echo ""
echo "▶ 게시 중…"
TAG="v$VERSION"
if ! git rev-parse "$TAG" >/dev/null 2>&1; then
    git tag "$TAG"
    git push origin "$TAG"
fi
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
    gh release upload "$TAG" "$ZIP" --repo "$REPO" --clobber
else
    gh release create "$TAG" "$ZIP" --repo "$REPO" \
        --title "eden $VERSION" --generate-notes
fi

# --- 9. Homebrew cask 갱신 ---
if [ -n "${TAP_DIR:-}" ]; then
    ./scripts/update-cask.sh "$TAP_DIR/Casks/eden.rb" "$VERSION" "$SHA"
    git -C "$TAP_DIR" add Casks/eden.rb
    git -C "$TAP_DIR" commit -m "eden $VERSION"
    git -C "$TAP_DIR" push
    echo "✅ cask 갱신·푸시 완료: $TAP_DIR/Casks/eden.rb"
fi

echo ""
echo "✅ 게시 완료 → brew install --cask kobums/tap/eden"
