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
  # 소스가 없으면 단색(터미널 배경색) 1024 정사각형 생성
  sips -s format png --resampleHeightWidth 1024 1024 \
    /System/Library/CoreServices/DefaultDesktop.heic --out "${BASE}" >/dev/null 2>&1 || \
    python3 - "${BASE}" <<'PY'
import sys, struct, zlib
w = h = 1024
bg = (22, 22, 30)
raw = b''.join(b'\x00' + bytes(bg) * w for _ in range(h))
def chunk(t, d):
    c = t + d
    return struct.pack('>I', len(d)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
png = b'\x89PNG\r\n\x1a\n'
png += chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
png += chunk(b'IDAT', zlib.compress(raw, 9))
png += chunk(b'IEND', b'')
open(sys.argv[1], 'wb').write(png)
PY
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
