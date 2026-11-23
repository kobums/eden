//! 셀 글자 스타일: 글꼴 변형(굵게·기울임)과 장식선(밑줄 5종·취소선).

use alacritty_terminal::term::cell::Flags;

/// 글리프 캐시를 나누는 글꼴 변형.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum FontStyle {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl FontStyle {
    pub(super) fn from_flags(flags: Flags) -> Self {
        match (flags.contains(Flags::BOLD), flags.contains(Flags::ITALIC)) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }

    pub(super) fn is_bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }

    pub(super) fn is_italic(self) -> bool {
        matches!(self, Self::Italic | Self::BoldItalic)
    }

    /// `styled_fonts` 배열 위치. Regular는 주 폰트라 없다.
    pub(super) fn face_index(self) -> Option<usize> {
        match self {
            Self::Regular => None,
            Self::Bold => Some(0),
            Self::Italic => Some(1),
            Self::BoldItalic => Some(2),
        }
    }
}

/// 주 폰트 파일 이름에서 같은 패밀리의 변형 파일 이름 후보를 만든다.
pub(super) fn sibling_names(file_name: &str, style: FontStyle) -> Vec<String> {
    let names: &[&str] = match style {
        FontStyle::Regular => &[],
        FontStyle::Bold => &["Bold"],
        FontStyle::Italic => &["Italic", "Oblique"],
        FontStyle::BoldItalic => &["Bold Italic", "BoldItalic", "BoldOblique"],
    };
    if !file_name.contains("Regular") {
        return Vec::new();
    }
    names
        .iter()
        .map(|name| file_name.replacen("Regular", name, 1))
        .collect()
}

/// 합성한 글리프 비트맵. `dx`는 원래 왼쪽 베어링에 더할 값.
#[derive(Debug, PartialEq)]
pub(super) struct Bitmap {
    pub(super) pixels: Vec<u8>,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) dx: i32,
}

/// 굵은 글꼴이 없는 폰트(한글 폴백 등)를 위해 가로로 `amount`px 두껍게 한다.
pub(super) fn embolden(src: &Bitmap, amount: usize) -> Bitmap {
    let width = src.width + amount;
    let mut pixels = vec![0u8; width * src.height];
    for y in 0..src.height {
        for x in 0..src.width {
            let value = src.pixels[y * src.width + x];
            for out in &mut pixels[y * width + x..=y * width + x + amount] {
                *out = (*out).max(value);
            }
        }
    }
    Bitmap {
        pixels,
        width,
        height: src.height,
        dx: src.dx,
    }
}

/// 기울임 글꼴이 없는 폰트를 위해 기준선을 축으로 비트맵을 민다.
pub(super) fn shear(src: &Bitmap, ymin: i32, slope: f32) -> Bitmap {
    if src.width == 0 || src.height == 0 {
        return Bitmap {
            pixels: Vec::new(),
            width: 0,
            height: 0,
            dx: src.dx,
        };
    }
    let shift_at = |row: usize| slope * ((src.height - row) as f32 - 0.5 + ymin as f32);
    let lowest = shift_at(src.height - 1);
    let highest = shift_at(0);
    let dx = lowest.floor() as i32;
    let width = src.width + (highest - dx as f32).ceil() as usize + 1;
    let mut pixels = vec![0u8; width * src.height];
    for y in 0..src.height {
        let offset = shift_at(y) - dx as f32;
        let whole = offset.floor() as usize;
        let frac = offset - whole as f32;
        for x in 0..src.width {
            let value = src.pixels[y * src.width + x] as f32;
            let left = y * width + x + whole;
            pixels[left] = (pixels[left] as f32 + value * (1.0 - frac)).min(255.0) as u8;
            pixels[left + 1] = (pixels[left + 1] as f32 + value * frac).min(255.0) as u8;
        }
    }
    Bitmap {
        pixels,
        width,
        height: src.height,
        dx: src.dx + dx,
    }
}

/// 장식선을 그릴 셀의 위치 (물리 픽셀).
#[derive(Clone, Copy)]
pub(super) struct DecorationCell {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) ascent: f32,
    pub(super) scale: f32,
}

/// 셀의 밑줄·취소선을 사각형들로 내보낸다 — `[x, y, w, h]`.
///
/// 점선과 물결은 화면 x좌표를 기준으로 무늬를 잡아 셀 경계에서 끊기지 않는다.
pub(super) fn decoration_rects(flags: Flags, cell: DecorationCell, mut emit: impl FnMut([f32; 4])) {
    let t = cell.scale.round().max(1.0);
    let bottom = cell.y + cell.height;
    let line_y = bottom - 2.0 * t;

    if flags.contains(Flags::UNDERCURL) {
        let period = (6.0 * t).max(4.0);
        let amplitude = t;
        let mut px = cell.x;
        while px < cell.x + cell.width {
            let phase = px / period * std::f32::consts::TAU;
            let y = line_y - amplitude + amplitude * phase.sin();
            let w = t.min(cell.x + cell.width - px);
            emit([px, y, w, t]);
            px += t;
        }
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        dashes(cell, line_y, t, t, t, &mut emit);
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        dashes(cell, line_y, t, 4.0 * t, 2.0 * t, &mut emit);
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        emit([cell.x, line_y, cell.width, t]);
        emit([cell.x, line_y - 2.0 * t, cell.width, t]);
    } else if flags.contains(Flags::UNDERLINE) {
        emit([cell.x, line_y, cell.width, t]);
    }

    if flags.contains(Flags::STRIKEOUT) {
        let y = cell.y + (cell.ascent * 0.68).round();
        emit([cell.x, y, cell.width, t]);
    }
}

fn dashes(
    cell: DecorationCell,
    y: f32,
    thickness: f32,
    on: f32,
    off: f32,
    emit: &mut impl FnMut([f32; 4]),
) {
    let period = on + off;
    let end = cell.x + cell.width;
    let mut start = (cell.x / period).floor() * period;
    while start < end {
        let left = start.max(cell.x);
        let right = (start + on).min(end);
        if right > left {
            emit([left, y, right - left, thickness]);
        }
        start += period;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: DecorationCell = DecorationCell {
        x: 20.0,
        y: 100.0,
        width: 10.0,
        height: 20.0,
        ascent: 15.0,
        scale: 2.0,
    };

    fn rects(flags: Flags) -> Vec<[f32; 4]> {
        let mut out = Vec::new();
        decoration_rects(flags, CELL, |r| out.push(r));
        out
    }

    #[test]
    fn style_comes_from_cell_flags() {
        assert_eq!(FontStyle::from_flags(Flags::empty()), FontStyle::Regular);
        assert_eq!(FontStyle::from_flags(Flags::BOLD), FontStyle::Bold);
        assert_eq!(FontStyle::from_flags(Flags::ITALIC), FontStyle::Italic);
        assert_eq!(
            FontStyle::from_flags(Flags::BOLD_ITALIC),
            FontStyle::BoldItalic
        );
    }

    #[test]
    fn sibling_font_names_follow_the_regular_file() {
        assert_eq!(
            sibling_names("MesloLGS NF Regular.ttf", FontStyle::BoldItalic)[0],
            "MesloLGS NF Bold Italic.ttf"
        );
        assert_eq!(
            sibling_names("Mono-Regular.otf", FontStyle::Italic),
            ["Mono-Italic.otf", "Mono-Oblique.otf"]
        );
        assert!(sibling_names("Monaco.ttf", FontStyle::Bold).is_empty());
    }

    #[test]
    fn embolden_widens_strokes() {
        let src = Bitmap {
            pixels: vec![0, 200, 0],
            width: 3,
            height: 1,
            dx: 2,
        };
        let out = embolden(&src, 1);
        assert_eq!(out.pixels, [0, 200, 200, 0]);
        assert_eq!(out.dx, 2);
    }

    #[test]
    fn shear_moves_top_rows_further_right() {
        let src = Bitmap {
            pixels: vec![255, 255, 255, 255],
            width: 1,
            height: 4,
            dx: 0,
        };
        let out = shear(&src, 0, 1.0);
        let center = |row: usize| {
            let line = &out.pixels[row * out.width..(row + 1) * out.width];
            let sum: f32 = line.iter().map(|&v| v as f32).sum();
            line.iter()
                .enumerate()
                .map(|(i, &v)| i as f32 * v as f32)
                .sum::<f32>()
                / sum
        };
        assert!(center(0) > center(1) && center(1) > center(3));
    }

    #[test]
    fn each_underline_kind_stays_inside_its_cell() {
        for flags in [
            Flags::UNDERLINE,
            Flags::DOUBLE_UNDERLINE,
            Flags::UNDERCURL,
            Flags::DOTTED_UNDERLINE,
            Flags::DASHED_UNDERLINE,
            Flags::STRIKEOUT,
        ] {
            let all = rects(flags);
            assert!(!all.is_empty(), "{flags:?}");
            for [x, y, w, h] in all {
                assert!(x >= 20.0 && x + w <= 30.0, "{flags:?}");
                assert!(y >= 100.0 && y + h <= 120.0, "{flags:?}");
            }
        }
    }

    #[test]
    fn double_has_two_lines_and_strike_adds_one() {
        assert_eq!(rects(Flags::DOUBLE_UNDERLINE).len(), 2);
        assert_eq!(rects(Flags::UNDERLINE | Flags::STRIKEOUT).len(), 2);
        assert!(rects(Flags::empty()).is_empty());
    }
}
