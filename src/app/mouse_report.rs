//! 마우스 리포팅 인코딩 — X10 / UTF-8 / SGR.
//!
//! TTY 앱(vim·htop·lazygit)이 DECSET 1000/1002/1003으로 마우스 리포팅을 켜면
//! 클릭·드래그·휠을 이스케이프 시퀀스로 보내야 한다. 이 모듈은 그 바이트를
//! 만드는 순수 함수만 담는다 — `App`도 `Term`도 락도 없어 단위 테스트가 된다.
//! winit 이벤트 처리와 락 관리는 `super::mouse`가 맡는다.

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

/// 리포트 종류. 버튼 코드의 기본값과 +32(모션) 여부를 결정한다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ReportKind {
    Press,
    Release,
    /// 버튼을 누른 채 이동(1002) 또는 그냥 이동(1003).
    Motion,
    WheelUp,
    WheelDown,
}

/// 버튼을 누르지 않은 채 이동할 때 쓰는 기본 버튼 코드.
const NO_BUTTON: u8 = 3;
/// 모션 비트.
const MOTION_BIT: u8 = 32;
/// 레거시 X10 인코딩의 좌표 오프셋. 32(공백)부터 시작해 1-based 좌표를 더한다.
const LEGACY_OFFSET: usize = 32;
/// 레거시 인코딩이 표현 가능한 최대 좌표값 (한 바이트 상한).
const LEGACY_MAX: usize = 255 - LEGACY_OFFSET;

/// 마우스 버튼 + 수정자 → 리포트 버튼 코드.
///
/// 좌 0 / 중 1 / 우 2, 휠은 64·65. 모션이면 +32.
/// 수정자 비트는 shift +4, alt +8, ctrl +16 (xterm 규약).
/// 지원하지 않는 버튼(Back/Forward 등)은 `None`.
pub(super) fn button_code(
    button: Option<MouseButton>,
    kind: ReportKind,
    mods: ModifiersState,
) -> Option<u8> {
    let base = match kind {
        ReportKind::WheelUp => 64,
        ReportKind::WheelDown => 65,
        _ => match button {
            Some(MouseButton::Left) => 0,
            Some(MouseButton::Middle) => 1,
            Some(MouseButton::Right) => 2,
            // 버튼 없이 이동하는 1003 모드에서는 "버튼 없음"을 뜻하는 3.
            None => NO_BUTTON,
            Some(_) => return None,
        },
    };

    let mut code = base;
    if kind == ReportKind::Motion {
        code += MOTION_BIT;
    }
    if mods.shift_key() {
        code += 4;
    }
    if mods.alt_key() {
        code += 8;
    }
    if mods.control_key() {
        code += 16;
    }
    Some(code)
}

/// 버튼 코드 + 0-based 뷰포트 좌표 → 리포트 바이트.
///
/// 인코딩은 터미널 모드가 고른다. SGR(1006)은 좌표 상한이 없고 해제 시
/// 버튼을 구분할 수 있어 현대 앱이 선호한다. 레거시 X10은 좌표를 한 바이트에
/// 담으므로 표현 범위를 넘으면 `None`을 돌려준다 — 쓰레기를 보내느니 버린다.
pub(super) fn encode(
    mode: TermMode,
    code: u8,
    col: usize,
    row: usize,
    press: bool,
) -> Option<Vec<u8>> {
    // 좌표는 1-based로 보고한다.
    let (x, y) = (col + 1, row + 1);

    if mode.contains(TermMode::SGR_MOUSE) {
        let terminator = if press { 'M' } else { 'm' };
        return Some(format!("\x1b[<{code};{x};{y}{terminator}").into_bytes());
    }

    // 레거시 계열은 해제를 버튼 3으로 뭉뚱그린다 (수정자 비트는 유지).
    let code = if press {
        code
    } else {
        (code & !0b11) | NO_BUTTON
    };

    if mode.contains(TermMode::UTF8_MOUSE) {
        let mut out = b"\x1b[M".to_vec();
        for v in [
            code as usize + LEGACY_OFFSET,
            x + LEGACY_OFFSET,
            y + LEGACY_OFFSET,
        ] {
            // UTF-8 모드 상한 2047 — char로 인코딩 가능한 범위를 넘으면 버린다.
            let ch = char::from_u32(v as u32)?;
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
        return Some(out);
    }

    if x > LEGACY_MAX || y > LEGACY_MAX {
        return None;
    }
    Some(vec![
        0x1b,
        b'[',
        b'M',
        code + LEGACY_OFFSET as u8,
        (x + LEGACY_OFFSET) as u8,
        (y + LEGACY_OFFSET) as u8,
    ])
}

/// 마우스 리포팅이 켜져 있고 사용자가 로컬 동작을 요구하지 않았는가.
///
/// Shift는 표준 탈출구다 — htop처럼 화면 전체를 쓰는 앱에서 텍스트를 선택할
/// 유일한 수단이므로 Shift가 눌려 있으면 리포팅하지 않는다. Cmd는
/// 하이퍼링크·블록 선택에 이미 쓰이므로 마찬가지로 제외한다.
pub(super) fn reporting_enabled(mode: TermMode, mods: ModifiersState) -> bool {
    if mods.shift_key() || mods.super_key() {
        return false;
    }
    mode.intersects(TermMode::MOUSE_MODE)
}

/// 이 커서 이동을 앱에 보고해야 하는가.
///
/// 1003(MOUSE_MOTION)은 버튼과 무관하게 모든 이동을, 1002(MOUSE_DRAG)는
/// 버튼을 누른 동안만 보고한다. 1000(클릭만)은 이동을 보고하지 않는다.
pub(super) fn should_report_motion(mode: TermMode, button_held: bool) -> bool {
    if mode.contains(TermMode::MOUSE_MOTION) {
        true
    } else if mode.contains(TermMode::MOUSE_DRAG) {
        button_held
    } else {
        false
    }
}

/// 물리 픽셀 → 0-based 뷰포트 셀 좌표와 셀 내 좌/우 반쪽.
///
/// 페인 밖 좌표는 가장자리 셀로 클램프한다 — 드래그가 페인을 벗어나도
/// 리포트와 선택이 계속 이어져야 하기 때문이다.
pub(super) fn cell_at(
    x: f64,
    y: f64,
    origin_x: f64,
    origin_y: f64,
    cell_w: f64,
    cell_h: f64,
    cols: usize,
    lines: usize,
) -> (usize, usize, Side) {
    let col = (((x - origin_x) / cell_w).floor().max(0.0) as usize).min(cols.saturating_sub(1));
    let row = (((y - origin_y) / cell_h).floor().max(0.0) as usize).min(lines.saturating_sub(1));

    let in_cell_x = (x - origin_x) - col as f64 * cell_w;
    let side = if in_cell_x < cell_w / 2.0 {
        Side::Left
    } else {
        Side::Right
    };
    (col, row, side)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, alt: bool, ctrl: bool) -> ModifiersState {
        let mut m = ModifiersState::empty();
        if shift {
            m |= ModifiersState::SHIFT;
        }
        if alt {
            m |= ModifiersState::ALT;
        }
        if ctrl {
            m |= ModifiersState::CONTROL;
        }
        m
    }

    const NONE: ModifiersState = ModifiersState::empty();

    #[test]
    fn buttons_map_to_their_codes() {
        let press = ReportKind::Press;
        assert_eq!(button_code(Some(MouseButton::Left), press, NONE), Some(0));
        assert_eq!(button_code(Some(MouseButton::Middle), press, NONE), Some(1));
        assert_eq!(button_code(Some(MouseButton::Right), press, NONE), Some(2));
        assert_eq!(button_code(None, press, NONE), Some(3), "버튼 없음");
    }

    #[test]
    fn unsupported_buttons_are_dropped() {
        let b = MouseButton::Other(9);
        assert_eq!(button_code(Some(b), ReportKind::Press, NONE), None);
        assert_eq!(
            button_code(Some(MouseButton::Back), ReportKind::Press, NONE),
            None
        );
    }

    #[test]
    fn wheel_ignores_the_button_and_uses_64_65() {
        assert_eq!(button_code(None, ReportKind::WheelUp, NONE), Some(64));
        assert_eq!(button_code(None, ReportKind::WheelDown, NONE), Some(65));
        assert_eq!(
            button_code(Some(MouseButton::Left), ReportKind::WheelUp, NONE),
            Some(64),
            "휠은 눌린 버튼과 무관"
        );
    }

    #[test]
    fn motion_adds_thirty_two() {
        assert_eq!(
            button_code(Some(MouseButton::Left), ReportKind::Motion, NONE),
            Some(32)
        );
        assert_eq!(
            button_code(None, ReportKind::Motion, NONE),
            Some(35),
            "버튼 없는 이동 = 3 + 32"
        );
    }

    #[test]
    fn modifier_bits_add_up() {
        let press = ReportKind::Press;
        let left = Some(MouseButton::Left);
        assert_eq!(button_code(left, press, mods(true, false, false)), Some(4));
        assert_eq!(button_code(left, press, mods(false, true, false)), Some(8));
        assert_eq!(button_code(left, press, mods(false, false, true)), Some(16));
        assert_eq!(
            button_code(left, press, mods(true, true, true)),
            Some(28),
            "4+8+16"
        );
        // 모션 비트와 함께
        assert_eq!(
            button_code(left, ReportKind::Motion, mods(true, false, false)),
            Some(36)
        );
    }

    #[test]
    fn sgr_encodes_one_based_coords_and_distinguishes_release() {
        let mode = TermMode::SGR_MOUSE;
        assert_eq!(
            encode(mode, 0, 11, 4, true).unwrap(),
            b"\x1b[<0;12;5M".to_vec(),
            "누름은 M으로 끝난다"
        );
        assert_eq!(
            encode(mode, 0, 11, 4, false).unwrap(),
            b"\x1b[<0;12;5m".to_vec(),
            "해제는 m으로 끝나고 버튼 코드를 유지한다"
        );
        // 우클릭 해제도 버튼을 구분할 수 있다 — 레거시와 결정적 차이.
        assert_eq!(
            encode(mode, 2, 0, 0, false).unwrap(),
            b"\x1b[<2;1;1m".to_vec()
        );
    }

    #[test]
    fn sgr_has_no_coordinate_ceiling() {
        let out = encode(TermMode::SGR_MOUSE, 0, 9999, 5000, true).unwrap();
        assert_eq!(out, b"\x1b[<0;10000;5001M".to_vec());
    }

    #[test]
    fn legacy_encodes_three_offset_bytes() {
        // col 11, row 4 → 1-based (12, 5) → 32+12=44, 32+5=37
        let out = encode(TermMode::empty(), 0, 11, 4, true).unwrap();
        assert_eq!(out, vec![0x1b, b'[', b'M', 32, 44, 37]);
    }

    #[test]
    fn legacy_release_collapses_to_button_three_but_keeps_modifiers() {
        // 좌클릭(0) 해제 → 버튼 3
        let out = encode(TermMode::empty(), 0, 0, 0, false).unwrap();
        assert_eq!(out[3], 32 + 3);

        // ctrl(+16) + 우클릭(2) 해제 → 3 | 16 = 19
        let out = encode(TermMode::empty(), 18, 0, 0, false).unwrap();
        assert_eq!(out[3], 32 + 19, "수정자 비트는 살아남는다");
    }

    #[test]
    fn legacy_drops_coordinates_past_its_one_byte_ceiling() {
        let mode = TermMode::empty();
        // 1-based 223이 마지막으로 표현 가능한 값 (32+223 = 255).
        assert!(encode(mode, 0, 222, 0, true).is_some(), "col 223은 가능");
        assert!(encode(mode, 0, 223, 0, true).is_none(), "col 224는 불가");
        assert!(encode(mode, 0, 0, 223, true).is_none(), "row도 동일");
    }

    #[test]
    fn utf8_mode_encodes_wide_coordinates_as_multibyte() {
        let mode = TermMode::UTF8_MOUSE;
        // 낮은 좌표는 레거시와 같은 바이트 (ASCII 범위)
        assert_eq!(
            encode(mode, 0, 11, 4, true).unwrap(),
            vec![0x1b, b'[', b'M', 32, 44, 37]
        );
        // 223을 넘어도 버리지 않고 UTF-8로 인코딩한다
        let out = encode(mode, 0, 300, 0, true).unwrap();
        assert!(out.len() > 6, "멀티바이트로 늘어난다");
        assert!(
            encode(mode, 0, 300, 0, true).is_some(),
            "레거시와 달리 표현 가능"
        );
    }

    #[test]
    fn reporting_is_off_unless_the_app_asked_for_it() {
        assert!(
            !reporting_enabled(TermMode::empty(), NONE),
            "모드가 꺼져 있으면 보고하지 않는다"
        );
        assert!(reporting_enabled(TermMode::MOUSE_REPORT_CLICK, NONE));
        assert!(reporting_enabled(TermMode::MOUSE_DRAG, NONE));
        assert!(reporting_enabled(TermMode::MOUSE_MOTION, NONE));
    }

    #[test]
    fn shift_and_cmd_keep_the_mouse_local() {
        let on = TermMode::MOUSE_REPORT_CLICK;
        assert!(
            !reporting_enabled(on, mods(true, false, false)),
            "Shift는 로컬 선택 탈출구 — htop에서 텍스트를 복사할 유일한 수단"
        );

        let mut cmd = ModifiersState::empty();
        cmd |= ModifiersState::SUPER;
        assert!(
            !reporting_enabled(on, cmd),
            "Cmd는 하이퍼링크·블록 선택에 쓰인다"
        );

        // alt·ctrl은 수정자 비트로 앱에 전달되므로 탈출구가 아니다.
        assert!(reporting_enabled(on, mods(false, true, false)));
        assert!(reporting_enabled(on, mods(false, false, true)));
    }

    #[test]
    fn motion_reporting_follows_the_mode() {
        // 1000: 클릭만 — 이동은 보고하지 않는다
        let click_only = TermMode::MOUSE_REPORT_CLICK;
        assert!(!should_report_motion(click_only, false));
        assert!(
            !should_report_motion(click_only, true),
            "버튼을 눌러도 1000은 이동을 원하지 않는다"
        );

        // 1002: 드래그 — 버튼을 누른 동안만
        let drag = TermMode::MOUSE_DRAG;
        assert!(!should_report_motion(drag, false), "버튼 없이는 보고 안 함");
        assert!(should_report_motion(drag, true));

        // 1003: 전체 이동 — 버튼과 무관
        let any = TermMode::MOUSE_MOTION;
        assert!(should_report_motion(any, false));
        assert!(should_report_motion(any, true));
    }

    #[test]
    fn cell_at_maps_pixels_to_cells() {
        // origin (8,8), 셀 10x20, 80x24 그리드
        let (col, row, _) = cell_at(8.0, 8.0, 8.0, 8.0, 10.0, 20.0, 80, 24);
        assert_eq!((col, row), (0, 0), "원점은 첫 셀");

        let (col, row, _) = cell_at(28.0, 48.0, 8.0, 8.0, 10.0, 20.0, 80, 24);
        assert_eq!((col, row), (2, 2));
    }

    #[test]
    fn cell_at_clamps_outside_the_pane() {
        let args = (8.0, 8.0, 10.0, 20.0, 80, 24);
        // 원점보다 왼쪽/위 → 0으로
        let (col, row, _) = cell_at(
            -500.0, -500.0, args.0, args.1, args.2, args.3, args.4, args.5,
        );
        assert_eq!((col, row), (0, 0));

        // 오른쪽/아래로 한참 밖 → 마지막 셀로
        let (col, row, _) = cell_at(
            9999.0, 9999.0, args.0, args.1, args.2, args.3, args.4, args.5,
        );
        assert_eq!((col, row), (79, 23));
    }

    #[test]
    fn cell_at_splits_the_cell_at_its_midpoint() {
        // 셀 폭 10 → 원점+0..5 는 Left, 5..10 은 Right
        let (_, _, side) = cell_at(8.0, 8.0, 8.0, 8.0, 10.0, 20.0, 80, 24);
        assert_eq!(side, Side::Left);

        let (_, _, side) = cell_at(12.9, 8.0, 8.0, 8.0, 10.0, 20.0, 80, 24);
        assert_eq!(side, Side::Left, "중점 직전");

        let (_, _, side) = cell_at(13.0, 8.0, 8.0, 8.0, 10.0, 20.0, 80, 24);
        assert_eq!(side, Side::Right, "정확히 중점부터 Right");
    }

    #[test]
    fn cell_at_survives_a_zero_sized_grid() {
        // 리사이즈 도중 0열/0줄이 들어와도 패닉하지 않아야 한다.
        let (col, row, _) = cell_at(100.0, 100.0, 0.0, 0.0, 10.0, 20.0, 0, 0);
        assert_eq!((col, row), (0, 0));
    }
}
