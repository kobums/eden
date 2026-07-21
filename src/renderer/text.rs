//! 글리프 아틀라스(래스터라이즈 + 캐싱)와 텍스트 한 줄 그리기.

use std::collections::HashMap;

use unicode_width::UnicodeWidthChar;

use super::{Renderer, TextInstance};

pub(super) const ATLAS_SIZE: u32 = 2048;

/// atlas에 캐싱된 글리프 하나.
#[derive(Clone, Copy)]
pub(super) struct Glyph {
    /// atlas 내 정규화 uv (x, y, w, h)
    pub(super) uv: [f32; 4],
    /// 물리 픽셀 크기
    pub(super) size: [f32; 2],
    /// 셀 원점 기준 오프셋 (x: 왼쪽 베어링, y: 셀 상단→글리프 상단)
    pub(super) offset: [f32; 2],
}

/// 글리프 크기 종류: 터미널 본문 / UI 크롬(탭·상태바, 더 작게).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum FontSize {
    Term,
    Ui,
}

pub(super) struct Atlas {
    pub(super) texture: wgpu::Texture,
    glyphs: HashMap<(char, FontSize), Option<Glyph>>,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
}

impl Atlas {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        Self {
            texture,
            glyphs: HashMap::new(),
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
        }
    }
}

impl Renderer {
    /// 터미널 본문 크기 글리프.
    pub(super) fn glyph(&mut self, c: char) -> Option<Glyph> {
        self.glyph_sized(c, FontSize::Term)
    }

    /// 글리프를 atlas에서 찾거나 새로 래스터라이즈해 올린다.
    /// 터미널 본문(Term)과 UI 크롬(Ui, 더 작은 크기) 두 크기를 캐싱한다.
    pub(super) fn glyph_sized(&mut self, c: char, size: FontSize) -> Option<Glyph> {
        let key = (c, size);
        if let Some(cached) = self.atlas.glyphs.get(&key) {
            return *cached;
        }
        let (px, ascent) = match size {
            FontSize::Term => (self.font_px, self.ascent),
            FontSize::Ui => (self.ui_px, self.ui_ascent),
        };

        // 주 폰트 → 폴백 폰트 순서로 글리프를 가진 폰트를 찾는다.
        // 단 PUA(사용자 영역, powerline 아이콘 등)는 폴백 폰트가 엉뚱한 글리프를
        // 돌려주는 경우가 있어 주 폰트에서만 찾는다.
        let is_pua = ('\u{E000}'..='\u{F8FF}').contains(&c);
        let font = if is_pua {
            self.fonts.first().filter(|f| f.lookup_glyph_index(c) != 0)
        } else {
            self.fonts.iter().find(|f| f.lookup_glyph_index(c) != 0)
        };
        let glyph = if let Some(font) = font {
            let (metrics, bitmap) = font.rasterize(c, px);
            if metrics.width == 0 || metrics.height == 0 {
                None
            } else {
                let w = metrics.width as u32;
                let h = metrics.height as u32;
                // shelf packing
                if self.atlas.cursor_x + w + 1 > ATLAS_SIZE {
                    self.atlas.cursor_x = 0;
                    self.atlas.cursor_y += self.atlas.row_height + 1;
                    self.atlas.row_height = 0;
                }
                if self.atlas.cursor_y + h + 1 > ATLAS_SIZE {
                    // atlas 가득 참 — 축출/증설은 아직 미구현
                    self.atlas.glyphs.insert(key, None);
                    return None;
                }
                let x = self.atlas.cursor_x;
                let y = self.atlas.cursor_y;
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.atlas.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x, y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &bitmap,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
                self.atlas.cursor_x += w + 1;
                self.atlas.row_height = self.atlas.row_height.max(h);

                let inv = 1.0 / ATLAS_SIZE as f32;
                Some(Glyph {
                    uv: [
                        x as f32 * inv,
                        y as f32 * inv,
                        w as f32 * inv,
                        h as f32 * inv,
                    ],
                    size: [metrics.width as f32, metrics.height as f32],
                    offset: [
                        metrics.xmin as f32,
                        // 셀 상단 → 글리프 상단: baseline(ascent) 위로 (height + ymin)만큼
                        ascent - metrics.height as f32 - metrics.ymin as f32,
                    ],
                })
            }
        } else {
            None
        };

        self.atlas.glyphs.insert(key, glyph);
        glyph
    }

    /// 한 글자가 차지하는 폭. 전각(한글·CJK)은 두 칸.
    pub(super) fn char_advance(&self, c: char, size: FontSize) -> f32 {
        let cell = match size {
            FontSize::Term => self.cell_width,
            FontSize::Ui => self.ui_advance,
        };
        if c.width().unwrap_or(1) >= 2 {
            cell * 2.0
        } else {
            cell
        }
    }

    /// 문자열 전체 폭 (우측 정렬·가운데 정렬 계산용).
    pub(super) fn text_width(&self, text: &str, size: FontSize) -> f32 {
        text.chars().map(|c| self.char_advance(c, size)).sum()
    }

    /// 텍스트 한 줄을 `(x, y)`부터 그린다. `max_x`를 넘는 글자는 그리지 않고 멈춘다.
    /// 다음 글자가 놓일 x를 돌려준다.
    pub(super) fn draw_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        max_x: f32,
        color: [f32; 3],
        size: FontSize,
        out: &mut Vec<TextInstance>,
    ) -> f32 {
        let mut cx = x;
        for ch in text.chars() {
            let advance = self.char_advance(ch, size);
            if cx + advance > max_x {
                break;
            }
            if let Some(glyph) = self.glyph_sized(ch, size) {
                out.push(TextInstance {
                    rect: [
                        cx + glyph.offset[0],
                        y + glyph.offset[1],
                        glyph.size[0],
                        glyph.size[1],
                    ],
                    uv: glyph.uv,
                    color: [color[0], color[1], color[2], 1.0],
                });
            }
            cx += advance;
        }
        cx
    }

    /// `max_w`에 맞게 자른 문자열과 그 폭. 잘리면 iTerm2처럼 말줄임표를 붙인다.
    pub(super) fn truncate_to_width(
        &self,
        text: &str,
        max_w: f32,
        size: FontSize,
    ) -> (String, f32) {
        let mut label = String::new();
        let mut width = 0.0;
        let mut truncated = false;
        for ch in text.chars() {
            let w = self.char_advance(ch, size);
            if width + w > max_w {
                truncated = true;
                break;
            }
            label.push(ch);
            width += w;
        }
        if truncated {
            const ELLIPSIS: char = '…';
            let ell_w = self.char_advance(ELLIPSIS, size);
            while width + ell_w > max_w {
                match label.pop() {
                    Some(c) => width -= self.char_advance(c, size),
                    None => break,
                }
            }
            label.push(ELLIPSIS);
            width += ell_w;
        }
        (label, width)
    }
}
