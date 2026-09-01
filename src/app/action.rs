//! 앱 액션과 키맵 (Phase 18).
//!
//! 키바인딩과 커맨드 팔레트가 **같은 액션 집합**을 공유한다. 예전에는 단축키
//! 표(`input::on_command_key`의 match)와 팔레트 액션 목록이 따로 있어, 액션을
//! 하나 추가하면 두 곳을 고쳐야 했고 조용히 갈라질 수 있었다.
//!
//! 키맵은 `(물리 키, Shift, Option)` → 액션의 조회 표다. 물리 키로 매칭하는
//! 이유는 Phase 15와 같다 — 한글 등 비라틴 입력 소스에서는 논리 키가
//! 자모("ㅅ")로 와서 문자 매칭이 실패한다.
//!
//! Cmd가 없는 조합은 다루지 않는다. 현재 단축키가 전부 Cmd 기반이고, Cmd 없는
//! 키는 PTY로 가야 하므로 사용자가 그 영역을 가로채면 셸이 망가진다.

use std::collections::HashMap;

use winit::keyboard::KeyCode;

/// 앱이 실행할 수 있는 동작. 설정에서는 kebab-case 이름으로 지정한다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Action {
    NewTab,
    ClosePane,
    SplitRight,
    SplitDown,
    ToggleZoom,
    NextTab,
    PrevTab,
    AiGenerate,
    Search,
    Palette,
    JumpPrevPrompt,
    JumpNextPrompt,
    Copy,
    CopyLastOutput,
    Paste,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    FontSizeUp,
    FontSizeDown,
    /// 폰트 크기를 설정 파일 값으로 되돌린다 (Cmd+0).
    FontSizeReset,
    /// 1-based 탭 번호로 전환 (Cmd+1..9).
    SelectTab(usize),
}

/// 설정 파일에 쓰는 액션 이름 ↔ 액션.
///
/// `select-tab-N`은 숫자가 붙어 표에 넣을 수 없으므로 [`Action::from_name`]에서
/// 따로 처리한다.
const ACTION_NAMES: &[(&str, Action)] = &[
    ("new-tab", Action::NewTab),
    ("close-pane", Action::ClosePane),
    ("split-right", Action::SplitRight),
    ("split-down", Action::SplitDown),
    ("toggle-zoom", Action::ToggleZoom),
    ("next-tab", Action::NextTab),
    ("prev-tab", Action::PrevTab),
    ("ai-generate", Action::AiGenerate),
    ("search", Action::Search),
    ("palette", Action::Palette),
    ("jump-prev-prompt", Action::JumpPrevPrompt),
    ("jump-next-prompt", Action::JumpNextPrompt),
    ("copy", Action::Copy),
    ("copy-last-output", Action::CopyLastOutput),
    ("paste", Action::Paste),
    ("focus-left", Action::FocusLeft),
    ("focus-right", Action::FocusRight),
    ("focus-up", Action::FocusUp),
    ("focus-down", Action::FocusDown),
    ("font-size-up", Action::FontSizeUp),
    ("font-size-down", Action::FontSizeDown),
    ("font-size-reset", Action::FontSizeReset),
];

impl Action {
    /// 설정의 액션 이름을 해석한다. 모르는 이름은 None (그 줄은 무시된다).
    pub(super) fn from_name(name: &str) -> Option<Action> {
        let name = name.trim().to_lowercase();
        if let Some(n) = name.strip_prefix("select-tab-") {
            let n: usize = n.parse().ok()?;
            return (1..=9).contains(&n).then_some(Action::SelectTab(n));
        }
        ACTION_NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, a)| *a)
    }
}

/// 키 조합. Cmd는 항상 필수라 필드로 두지 않는다.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) struct Chord {
    pub(super) code: KeyCode,
    pub(super) shift: bool,
    pub(super) alt: bool,
}

/// 기본 바인딩표. `shift`가 `None`이면 Shift 유무와 무관하게 매칭한다.
///
/// 예전 `on_command_key`의 match에는 Shift를 보지 않는 팔(`KeyCode::KeyF`,
/// `KeyZ`, 숫자, 화살표)이 있었다. 표로 옮기면서 정확히 그 동작을 유지하려고
/// `None`을 두 항목(Shift 있음/없음)으로 펼친다 — 안 그러면 Cmd+Shift+F가
/// 조용히 죽어 회귀가 된다.
#[rustfmt::skip]
const DEFAULTS: &[(KeyCode, Option<bool>, bool, Action)] = &[
    // (키, Shift, Option, 액션)
    (KeyCode::KeyP,         Some(true),  false, Action::Palette),
    (KeyCode::KeyK,         Some(false), false, Action::AiGenerate),
    (KeyCode::KeyF,         None,        false, Action::Search),
    (KeyCode::KeyT,         Some(false), false, Action::NewTab),
    (KeyCode::KeyW,         Some(false), false, Action::ClosePane),
    (KeyCode::BracketRight, Some(true),  false, Action::NextTab),
    (KeyCode::BracketLeft,  Some(true),  false, Action::PrevTab),
    (KeyCode::KeyD,         Some(true),  false, Action::SplitDown),
    (KeyCode::KeyD,         Some(false), false, Action::SplitRight),
    (KeyCode::KeyZ,         None,        false, Action::ToggleZoom),
    (KeyCode::KeyC,         Some(true),  false, Action::CopyLastOutput),
    (KeyCode::KeyC,         Some(false), false, Action::Copy),
    (KeyCode::KeyV,         Some(false), false, Action::Paste),
    (KeyCode::ArrowUp,      None,        false, Action::JumpPrevPrompt),
    (KeyCode::ArrowDown,    None,        false, Action::JumpNextPrompt),
    // Cmd+Option+화살표: 페인 포커스 이동
    (KeyCode::ArrowLeft,    None,        true,  Action::FocusLeft),
    (KeyCode::ArrowRight,   None,        true,  Action::FocusRight),
    (KeyCode::ArrowUp,      None,        true,  Action::FocusUp),
    (KeyCode::ArrowDown,    None,        true,  Action::FocusDown),
    // Cmd+= / Cmd+- / Cmd+0: 폰트 크기. Shift 무관 — Cmd+Shift+=(즉 Cmd++)도
    // 크기 키우기로 받는 것이 모든 터미널의 관례다.
    (KeyCode::Equal,        None,        false, Action::FontSizeUp),
    (KeyCode::Minus,        None,        false, Action::FontSizeDown),
    (KeyCode::Digit0,       None,        false, Action::FontSizeReset),
    // Cmd+1..9: 탭 전환
    (KeyCode::Digit1,       None,        false, Action::SelectTab(1)),
    (KeyCode::Digit2,       None,        false, Action::SelectTab(2)),
    (KeyCode::Digit3,       None,        false, Action::SelectTab(3)),
    (KeyCode::Digit4,       None,        false, Action::SelectTab(4)),
    (KeyCode::Digit5,       None,        false, Action::SelectTab(5)),
    (KeyCode::Digit6,       None,        false, Action::SelectTab(6)),
    (KeyCode::Digit7,       None,        false, Action::SelectTab(7)),
    (KeyCode::Digit8,       None,        false, Action::SelectTab(8)),
    (KeyCode::Digit9,       None,        false, Action::SelectTab(9)),
];

/// 조합 → 액션 조회 표.
pub(super) struct Keymap(HashMap<Chord, Action>);

impl Keymap {
    /// 기본 맵에 사용자 재정의를 덮어쓴다.
    ///
    /// 덮어쓰기는 **조합 단위**다 — `keybind = cmd+t = split-right` 한 줄은
    /// Cmd+T만 바꾸고 나머지 기본 바인딩은 그대로 둔다. 표 전체를 교체하면
    /// 설정 한 줄 때문에 모든 단축키를 다시 적어야 한다.
    ///
    /// 해석할 수 없는 줄(모르는 키·액션, Cmd 없음)은 조용히 무시한다 —
    /// 설정 파일의 다른 실수를 다루는 방식과 같다.
    pub(super) fn from_config(overrides: &[(String, String)]) -> Self {
        let mut map = HashMap::new();
        for &(code, shift, alt, action) in DEFAULTS {
            // Shift 무관(None) 항목은 두 갈래로 펼친다.
            let variants: &[bool] = match shift {
                Some(true) => &[true],
                Some(false) => &[false],
                None => &[false, true],
            };
            for &shift in variants {
                map.insert(Chord { code, shift, alt }, action);
            }
        }
        for (chord, name) in overrides {
            if let (Some(chord), Some(action)) = (parse_chord(chord), Action::from_name(name)) {
                map.insert(chord, action);
            }
        }
        Self(map)
    }

    pub(super) fn get(&self, chord: Chord) -> Option<Action> {
        self.0.get(&chord).copied()
    }
}

/// `cmd+shift+d` 같은 설정 문자열을 조합으로 해석한다.
///
/// Cmd가 없으면 None — PTY로 가야 할 키를 앱이 가로채지 않게 하는 방어선이다.
fn parse_chord(text: &str) -> Option<Chord> {
    let mut cmd = false;
    let mut shift = false;
    let mut alt = false;
    let mut code = None;

    for part in text.split('+') {
        match part.trim().to_lowercase().as_str() {
            "" => continue,
            "cmd" | "super" | "command" => cmd = true,
            "shift" => shift = true,
            "opt" | "alt" | "option" => alt = true,
            key => {
                // 키 이름이 둘 이상이면 잘못된 줄이다 (`cmd+a+b`).
                if code.is_some() {
                    return None;
                }
                code = Some(parse_key(key)?);
            }
        }
    }
    cmd.then_some(Chord {
        code: code?,
        shift,
        alt,
    })
}

/// 설정에 쓰는 키 이름 → 물리 키.
fn parse_key(name: &str) -> Option<KeyCode> {
    // 글자·숫자는 표 없이 계산한다 (35줄짜리 match를 피한다).
    let bytes = name.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0];
        if c.is_ascii_lowercase() {
            return LETTERS.get((c - b'a') as usize).copied();
        }
        if c.is_ascii_digit() {
            return DIGITS.get((c - b'0') as usize).copied();
        }
    }
    Some(match name {
        "left" => KeyCode::ArrowLeft,
        "right" => KeyCode::ArrowRight,
        "up" => KeyCode::ArrowUp,
        "down" => KeyCode::ArrowDown,
        "[" | "bracketleft" => KeyCode::BracketLeft,
        "]" | "bracketright" => KeyCode::BracketRight,
        "enter" | "return" => KeyCode::Enter,
        "space" => KeyCode::Space,
        "tab" => KeyCode::Tab,
        "=" | "equal" | "plus" => KeyCode::Equal,
        "-" | "minus" => KeyCode::Minus,
        _ => return None,
    })
}

#[rustfmt::skip]
const LETTERS: [KeyCode; 26] = [
    KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyC, KeyCode::KeyD, KeyCode::KeyE,
    KeyCode::KeyF, KeyCode::KeyG, KeyCode::KeyH, KeyCode::KeyI, KeyCode::KeyJ,
    KeyCode::KeyK, KeyCode::KeyL, KeyCode::KeyM, KeyCode::KeyN, KeyCode::KeyO,
    KeyCode::KeyP, KeyCode::KeyQ, KeyCode::KeyR, KeyCode::KeyS, KeyCode::KeyT,
    KeyCode::KeyU, KeyCode::KeyV, KeyCode::KeyW, KeyCode::KeyX, KeyCode::KeyY,
    KeyCode::KeyZ,
];

#[rustfmt::skip]
const DIGITS: [KeyCode; 10] = [
    KeyCode::Digit0, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4,
    KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(code: KeyCode, shift: bool, alt: bool) -> Chord {
        Chord { code, shift, alt }
    }

    fn default_map() -> Keymap {
        Keymap::from_config(&[])
    }

    // ─── 액션 이름 ───

    #[test]
    fn action_names_round_trip() {
        for (name, action) in ACTION_NAMES {
            assert_eq!(Action::from_name(name), Some(*action), "{name}");
        }
    }

    #[test]
    fn select_tab_is_range_checked() {
        assert_eq!(
            Action::from_name("select-tab-1"),
            Some(Action::SelectTab(1))
        );
        assert_eq!(
            Action::from_name("select-tab-9"),
            Some(Action::SelectTab(9))
        );
        assert_eq!(Action::from_name("select-tab-0"), None, "탭은 1부터");
        assert_eq!(Action::from_name("select-tab-10"), None);
        assert_eq!(Action::from_name("select-tab-x"), None);
    }

    #[test]
    fn unknown_action_names_are_rejected() {
        assert_eq!(Action::from_name("nonexistent"), None);
        assert_eq!(Action::from_name(""), None);
    }

    #[test]
    fn action_names_are_case_insensitive_and_trimmed() {
        assert_eq!(Action::from_name("  New-Tab "), Some(Action::NewTab));
    }

    // ─── 조합 파싱 ───

    #[test]
    fn parses_basic_chords() {
        assert_eq!(
            parse_chord("cmd+t"),
            Some(chord(KeyCode::KeyT, false, false))
        );
        assert_eq!(
            parse_chord("cmd+shift+d"),
            Some(chord(KeyCode::KeyD, true, false))
        );
        assert_eq!(
            parse_chord("cmd+opt+left"),
            Some(chord(KeyCode::ArrowLeft, false, true))
        );
        assert_eq!(
            parse_chord("cmd+3"),
            Some(chord(KeyCode::Digit3, false, false))
        );
    }

    #[test]
    fn modifier_order_and_case_do_not_matter() {
        let want = Some(chord(KeyCode::KeyD, true, true));
        assert_eq!(parse_chord("cmd+shift+opt+d"), want);
        assert_eq!(parse_chord("opt+shift+cmd+d"), want);
        assert_eq!(parse_chord("CMD+Shift+ALT+D"), want);
        assert_eq!(parse_chord("command+shift+option+d"), want);
    }

    #[test]
    fn chords_without_cmd_are_rejected() {
        // Cmd 없는 키는 PTY로 가야 한다 — 앱이 가로채면 셸이 망가진다.
        assert_eq!(parse_chord("ctrl+a"), None);
        assert_eq!(parse_chord("shift+t"), None);
        assert_eq!(parse_chord("t"), None);
    }

    #[test]
    fn malformed_chords_are_rejected() {
        assert_eq!(parse_chord("cmd"), None, "키가 없다");
        assert_eq!(parse_chord("cmd+nonexistent"), None);
        assert_eq!(parse_chord("cmd+a+b"), None, "키가 둘");
        assert_eq!(parse_chord(""), None);
    }

    // ─── 기본 맵이 예전 단축키 표와 같은가 (회귀 스냅샷) ───

    #[test]
    fn default_map_matches_the_previous_shortcut_table() {
        let m = default_map();
        let cases = [
            (chord(KeyCode::KeyP, true, false), Action::Palette),
            (chord(KeyCode::KeyK, false, false), Action::AiGenerate),
            (chord(KeyCode::KeyT, false, false), Action::NewTab),
            (chord(KeyCode::KeyW, false, false), Action::ClosePane),
            (chord(KeyCode::BracketRight, true, false), Action::NextTab),
            (chord(KeyCode::BracketLeft, true, false), Action::PrevTab),
            (chord(KeyCode::KeyD, true, false), Action::SplitDown),
            (chord(KeyCode::KeyD, false, false), Action::SplitRight),
            (chord(KeyCode::KeyC, true, false), Action::CopyLastOutput),
            (chord(KeyCode::KeyC, false, false), Action::Copy),
            (chord(KeyCode::KeyV, false, false), Action::Paste),
            (chord(KeyCode::ArrowLeft, false, true), Action::FocusLeft),
            (chord(KeyCode::ArrowRight, false, true), Action::FocusRight),
            (chord(KeyCode::ArrowUp, false, true), Action::FocusUp),
            (chord(KeyCode::ArrowDown, false, true), Action::FocusDown),
            (chord(KeyCode::Digit1, false, false), Action::SelectTab(1)),
            (chord(KeyCode::Digit9, false, false), Action::SelectTab(9)),
            (chord(KeyCode::Equal, false, false), Action::FontSizeUp),
            (chord(KeyCode::Minus, false, false), Action::FontSizeDown),
            (chord(KeyCode::Digit0, false, false), Action::FontSizeReset),
        ];
        for (c, want) in cases {
            assert_eq!(m.get(c), Some(want), "{c:?}");
        }
    }

    #[test]
    fn font_size_chords_are_shift_agnostic() {
        // Cmd++는 물리적으로 Cmd+Shift+= 이다 — Shift가 있어도 크기 키우기.
        let m = default_map();
        for shift in [false, true] {
            assert_eq!(
                m.get(chord(KeyCode::Equal, shift, false)),
                Some(Action::FontSizeUp)
            );
            assert_eq!(
                m.get(chord(KeyCode::Minus, shift, false)),
                Some(Action::FontSizeDown)
            );
        }
    }

    #[test]
    fn font_size_keys_parse_in_keybind_lines() {
        assert_eq!(
            parse_chord("cmd+="),
            Some(chord(KeyCode::Equal, false, false))
        );
        assert_eq!(
            parse_chord("cmd+plus"),
            Some(chord(KeyCode::Equal, false, false))
        );
        assert_eq!(
            parse_chord("cmd+minus"),
            Some(chord(KeyCode::Minus, false, false))
        );
        // `cmd+-`는 split('+')에서 빈 조각이 생기지만 "-" 조각이 살아남는다.
        assert_eq!(
            parse_chord("cmd+-"),
            Some(chord(KeyCode::Minus, false, false))
        );
    }

    #[test]
    fn shift_agnostic_defaults_match_with_and_without_shift() {
        // 예전 match의 catch-all 팔들 — Shift를 보지 않았다.
        let m = default_map();
        for shift in [false, true] {
            assert_eq!(
                m.get(chord(KeyCode::KeyF, shift, false)),
                Some(Action::Search)
            );
            assert_eq!(
                m.get(chord(KeyCode::KeyZ, shift, false)),
                Some(Action::ToggleZoom)
            );
            assert_eq!(
                m.get(chord(KeyCode::Digit1, shift, false)),
                Some(Action::SelectTab(1))
            );
            assert_eq!(
                m.get(chord(KeyCode::ArrowUp, shift, false)),
                Some(Action::JumpPrevPrompt)
            );
        }
    }

    #[test]
    fn unbound_chords_return_nothing() {
        let m = default_map();
        assert_eq!(m.get(chord(KeyCode::KeyQ, false, false)), None);
        // Shift를 명시한 기본 항목은 반대쪽이 비어 있다.
        assert_eq!(m.get(chord(KeyCode::KeyT, true, false)), None);
        // Option 조합은 화살표에만 있다.
        assert_eq!(m.get(chord(KeyCode::KeyT, false, true)), None);
    }

    // ─── 사용자 재정의 ───

    #[test]
    fn an_override_replaces_only_that_one_chord() {
        let m = Keymap::from_config(&[("cmd+t".into(), "split-right".into())]);
        assert_eq!(
            m.get(chord(KeyCode::KeyT, false, false)),
            Some(Action::SplitRight),
            "재정의된 조합"
        );
        assert_eq!(
            m.get(chord(KeyCode::KeyD, false, false)),
            Some(Action::SplitRight),
            "기본 바인딩은 그대로 남는다"
        );
        assert_eq!(
            m.get(chord(KeyCode::KeyK, false, false)),
            Some(Action::AiGenerate),
            "무관한 바인딩도 그대로"
        );
    }

    #[test]
    fn an_override_can_add_a_new_chord() {
        let m = Keymap::from_config(&[("cmd+shift+t".into(), "new-tab".into())]);
        assert_eq!(
            m.get(chord(KeyCode::KeyT, true, false)),
            Some(Action::NewTab)
        );
        assert_eq!(
            m.get(chord(KeyCode::KeyT, false, false)),
            Some(Action::NewTab),
            "기본 Cmd+T도 살아 있다"
        );
    }

    #[test]
    fn bad_override_lines_are_ignored_without_breaking_the_rest() {
        let m = Keymap::from_config(&[
            ("ctrl+a".into(), "new-tab".into()),    // Cmd 없음
            ("cmd+t".into(), "nonexistent".into()), // 모르는 액션
            ("cmd+nope".into(), "new-tab".into()),  // 모르는 키
            ("cmd+k".into(), "toggle-zoom".into()), // 정상
        ]);
        assert_eq!(
            m.get(chord(KeyCode::KeyT, false, false)),
            Some(Action::NewTab),
            "잘못된 줄은 기본값을 건드리지 않는다"
        );
        assert_eq!(
            m.get(chord(KeyCode::KeyK, false, false)),
            Some(Action::ToggleZoom),
            "뒤따르는 정상 줄은 적용된다"
        );
    }

    #[test]
    fn overriding_a_shift_agnostic_default_touches_only_the_named_variant() {
        // Cmd+F는 기본이 Shift 무관 두 항목이다. `cmd+f` 재정의는 그중
        // Shift 없는 쪽만 바꾼다 — 문서화된 동작.
        let m = Keymap::from_config(&[("cmd+f".into(), "new-tab".into())]);
        assert_eq!(
            m.get(chord(KeyCode::KeyF, false, false)),
            Some(Action::NewTab)
        );
        assert_eq!(
            m.get(chord(KeyCode::KeyF, true, false)),
            Some(Action::Search),
            "Cmd+Shift+F는 그대로 검색"
        );
    }
}
