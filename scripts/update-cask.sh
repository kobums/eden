#!/bin/bash
# Homebrew cask 파일을 버전·sha256 으로 재생성한다.
# 사용: ./scripts/update-cask.sh <출력경로> <version> <sha256>
# release.sh --publish 가 자동 호출하며, tap 저장소 갱신용으로 단독 실행도 가능하다.
set -euo pipefail

OUT="$1"; VERSION="$2"; SHA="$3"

mkdir -p "$(dirname "$OUT")"
cat > "$OUT" <<CASK
cask "eden" do
  version "$VERSION"
  sha256 "$SHA"

  url "https://github.com/kobums/eden/releases/download/v#{version}/eden-#{version}.zip"
  name "eden"
  desc "여러 터미널의 장점을 모은 macOS 네이티브 터미널"
  homepage "https://github.com/kobums/eden"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: :big_sur

  app "eden.app"

  zap trash: [
    "~/.config/eden",
    "~/.cache/eden",
  ]
end
CASK

echo "cask 작성: $OUT (version $VERSION)"
