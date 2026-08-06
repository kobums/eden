# Homebrew Cask 템플릿.
# 배포 시 version/sha256/url을 실제 GitHub 릴리스에 맞게 채운다.
#   brew install --cask ./Casks/eden.rb   (로컬 테스트)
cask "eden" do
  version "0.1.0"
  sha256 :no_check # 릴리스 후 실제 zip의 sha256으로 교체

  url "https://github.com/kobums/eden/releases/download/v#{version}/eden-#{version}.zip"
  name "eden"
  desc "여러 터미널의 장점을 모은 macOS 네이티브 터미널"
  homepage "https://github.com/kobums/eden"

  depends_on macos: ">= :big_sur"

  app "eden.app"

  zap trash: [
    "~/.config/eden",
    "~/.cache/eden",
  ]
end
