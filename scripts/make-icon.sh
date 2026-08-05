#!/bin/bash
# 간단한 앱 아이콘(dist/AppIcon.icns)을 생성한다. 소스 PNG가 있으면 그걸 쓰고,
# 없으면 단색 배경의 기본 아이콘을 만든다.
#   ./scripts/make-icon.sh [source.png]
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p dist

SRC="${1:-}"
TMP="$(mktemp -d)"
BASE="${TMP}/base.png"

if [[ -n "${SRC}" && -f "${SRC}" ]]; then
  sips -z 1024 1024 "${SRC}" --out "${BASE}" >/dev/null
else
  # 소스가 없으면 기본 아이콘을 그린다: 다크 스퀘어클 + 초록 ◡̈ 스마일
  cat > "${TMP}/icon.swift" <<'SWIFT'
import AppKit

let out = CommandLine.arguments[1]
let size: CGFloat = 1024

let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

// macOS 아이콘 그리드: 1024 캔버스에 여백을 두고 824 스퀘어클
let inset: CGFloat = 100
let rect = NSRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
NSColor(srgbRed: 0.105, green: 0.105, blue: 0.115, alpha: 1).setFill()
NSBezierPath(roundedRect: rect, xRadius: 185, yRadius: 185).fill()

// ◡ (U+25E1) + 결합 분음 부호(U+0308) — 스마일
let fontSize: CGFloat = 520
let font = NSFont(name: "Menlo", size: fontSize) ?? NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
let attrs: [NSAttributedString.Key: Any] = [
    .font: font,
    .foregroundColor: NSColor(srgbRed: 0.30, green: 0.85, blue: 0.42, alpha: 1),
]
let str = NSAttributedString(string: "\u{25E1}\u{0308}", attributes: attrs)
// 글리프 실제 잉크 영역으로 정확히 가운데 정렬
let line = CTLineCreateWithAttributedString(str)
let ctx = NSGraphicsContext.current!.cgContext
let bounds = CTLineGetImageBounds(line, ctx)
ctx.textPosition = CGPoint(
    x: (size - bounds.width) / 2 - bounds.minX,
    y: (size - bounds.height) / 2 - bounds.minY)
CTLineDraw(line, ctx)

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
SWIFT
  swift "${TMP}/icon.swift" "${BASE}"
fi

ICONSET="${TMP}/AppIcon.iconset"
mkdir -p "${ICONSET}"
for size in 16 32 128 256 512; do
  sips -z $size $size       "${BASE}" --out "${ICONSET}/icon_${size}x${size}.png" >/dev/null
  sips -z $((size*2)) $((size*2)) "${BASE}" --out "${ICONSET}/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "${ICONSET}" -o dist/AppIcon.icns
rm -rf "${TMP}"
echo "==> dist/AppIcon.icns 생성됨"
