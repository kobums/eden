//! Kitty keyboard protocol (CSI u) 키 인코더.
//!
//! 프로토콜 상태 머신(push/pop/query, TermMode 플래그 5종, 대체 스크린 분리)은
//! alacritty_terminal이 내장한다. 이 모듈은 "현재 모드에서 이 키를 어떤
//! 바이트로 보낼 것인가"만 담는다 — mouse_report.rs와 같은 원칙으로 `App`도
//! `Term`도 락도 없는 순수 함수라 전체를 단위 테스트로 덮는다.
//!
//! 기준 구현은 kitty 자신의 key_encoding.c다. alacritty 0.15의 인코더도
//! 참고했지만 한 곳에서 의도적으로 갈라선다: DISAMBIGUATE 모드에서 kitty는
//! Shift+Enter·Shift+Tab처럼 "수식자 붙은 Enter/Tab/Backspace"를 CSI u로
//! 보내는데 alacritty는 레거시로 남긴다. 스펙의 레거시 예외("`reset`을 칠 수
//! 있어야 한다")는 수식자 없는 경우에만 적용되는 것이고, Neovim의 <S-CR>
//! 매핑이 되려면 kitty 쪽을 따라야 한다.

use std::fmt::Write as _;

use alacritty_terminal::term::TermMode;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyLocation, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

/// 인코더 입력. winit `KeyEvent`는 필드가 비공개라 테스트에서 만들 수 없으므로
/// 필요한 값만 뽑은 자체 타입을 쓴다 (mouse_report.rs가 좌표·버튼을 원시 값으로
/// 받는 것과 같은 해법). winit → 이 타입 변환은 `from_winit` 어댑터 한 곳뿐이다.
pub(super) struct KeyInput {
    pub logical_key: Key,
    /// 수식자를 전부 뗀 키. Shift+1이 '!'로 올 때 기본 키 '1'(코드 49)을
    /// 복원하는 데 쓴다 (REPORT_ALTERNATE_KEYS의 unicode-key-code).
    pub key_without_modifiers: Key,
    pub location: KeyLocation,
    pub state: ElementState,
    pub repeat: bool,
    /// Ctrl까지 반영된 생성 텍스트(`text_with_all_modifiers`). `KeyEvent::text`
    /// 대신 이걸 쓰는 이유: Ctrl+A의 텍스트가 "\x01"(제어 문자)로 와야
    /// "텍스트를 만드는 키"와 "제어 조합"을 구분할 수 있다.
    pub text: Option<String>,
}

impl KeyInput {
    pub(super) fn from_winit(event: &KeyEvent) -> Self {
        Self {
            logical_key: event.logical_key.clone(),
            key_without_modifiers: event.key_without_modifiers(),
            location: event.location,
            state: event.state,
            repeat: event.repeat,
            text: event.text_with_all_modifiers().map(str::to_owned),
        }
    }
}

/// 인코딩 판정 결과.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Encoded {
    /// 이 바이트를 PTY로 보낸다.
    Bytes(Vec<u8>),
    /// 이 키는 레거시 경로(`key_to_bytes`)가 처리한다 — kitty 모드에서도
    /// 텍스트를 만드는 키는 평문으로 남는 경우가 많다.
    Legacy,
    /// 아무것도 보내지 않는다 (보고 대상이 아닌 release·수식키 단독 등).
    Nothing,
}

/// 키 입력 하나를 현재 터미널 모드에 따라 판정한다. press·release 공용.
pub(super) fn encode(input: &KeyInput, mods: ModifiersState, mode: TermMode) -> Encoded {
    let release = input.state == ElementState::Released;

    // 디스패치(어떤 키를 CSI로 보낼지)를 바꾸는 플래그는 DISAMBIGUATE·
    // REPORT_EVENT_TYPES·REPORT_ALL_KEYS_AS_ESC 셋뿐이다. ALTERNATE_KEYS·
    // ASSOCIATED_TEXT는 다른 플래그가 만든 시퀀스에 필드를 덧붙일 뿐이라
    // 단독으로는 전부 레거시다 (kitty key_encoding.c와 동일한 판정).
    if !mode.intersects(
        TermMode::DISAMBIGUATE_ESC_CODES
            | TermMode::REPORT_EVENT_TYPES
            | TermMode::REPORT_ALL_KEYS_AS_ESC,
    ) {
        return if release {
            Encoded::Nothing
        } else {
            Encoded::Legacy
        };
    }
    // release는 REPORT_EVENT_TYPES가 있어야만 존재하는 개념이다.
    if release && !mode.contains(TermMode::REPORT_EVENT_TYPES) {
        return Encoded::Nothing;
    }

    let encode_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let mods = effective_mods(input, mods);

    // 수식키 단독(Shift 등)은 REPORT_ALL_KEYS_AS_ESC에서만 보고한다.
    if is_modifier_key(&input.logical_key) && !encode_all {
        return Encoded::Nothing;
    }

    if !encode_all {
        // 텍스트를 만드는 press/repeat는 평문 그대로 — kitty의
        // SEND_TEXT_TO_CHILD. 제어 문자 텍스트(Ctrl+A의 \x01, Enter의 \r 등)는
        // "텍스트"로 치지 않으므로 아래 시퀀스 경로로 내려간다. 단 numpad는
        // DISAMBIGUATE부터 전용 코드포인트로 보고해야 하므로 예외.
        if !release
            && input.location != KeyLocation::Numpad
            && input
                .text
                .as_deref()
                .is_some_and(|t| !t.is_empty() && !is_control_text(t))
        {
            return Encoded::Legacy;
        }
        // 수식자 없는 Enter/Tab/Backspace는 레거시 바이트 유지 — 이 모드를 못
        // 끄고 죽은 앱 뒤에서도 셸에 `reset`을 칠 수 있어야 한다는 스펙의
        // 예외. 이 세 키의 release는 보고하지 않는다 (kitty와 동일).
        if mods.is_empty()
            && input.location != KeyLocation::Numpad
            && matches!(
                input.logical_key,
                Key::Named(NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace)
            )
        {
            return if release {
                Encoded::Nothing
            } else {
                Encoded::Legacy
            };
        }
        // Esc는 DISAMBIGUATE가 없으면(REPORT_EVENT_TYPES만이면) press를 레거시
        // \x1b로 남긴다. release는 kitty가 raw \x1b를 또 보내지만 그건 앱을
        // 혼란시키므로 alacritty처럼 CSI 27 release로 보낸다.
        if !release
            && mods.is_empty()
            && !mode.contains(TermMode::DISAMBIGUATE_ESC_CODES)
            && input.logical_key == Key::Named(NamedKey::Escape)
        {
            return Encoded::Legacy;
        }
    }

    match build_sequence(input, mods, mode) {
        Some(bytes) => Encoded::Bytes(bytes),
        // 시퀀스로 표현 못 하는 키(Dead 키 등): press는 레거시에 맡기고
        // release는 버린다 — release를 레거시로 흘리면 텍스트가 중복된다.
        None if release => Encoded::Nothing,
        None => Encoded::Legacy,
    }
}

/// macOS에서 Option은 문자 조합(å 등)에 쓰이므로, 조합 텍스트를 만든 키에서는
/// ALT를 수식자로 보고하지 않는다 (kitty의 macos_option_as_alt=no와 동일).
/// 텍스트가 없거나 제어 문자인 키(화살표·기능키·Ctrl 조합)에서는 수식자로
/// 남긴다.
fn effective_mods(input: &KeyInput, mods: ModifiersState) -> ModifiersState {
    if !mods.alt_key() {
        return mods;
    }
    let keeps_alt = match &input.logical_key {
        Key::Named(named) => named.to_text().is_none(),
        _ => input
            .text
            .as_deref()
            .is_none_or(|t| t.is_empty() || is_control_text(t)),
    };
    if keeps_alt {
        mods
    } else {
        mods & !ModifiersState::ALT
    }
}

/// 수식키 자체인가. Cmd 단축키 분기(`on_key`)와 REPORT_ALL_KEYS_AS_ESC 판정
/// 양쪽에서 쓴다.
pub(super) fn is_modifier_key(key: &Key) -> bool {
    matches!(
        key,
        Key::Named(
            NamedKey::Shift
                | NamedKey::Control
                | NamedKey::Alt
                | NamedKey::AltGraph
                | NamedKey::Super
                | NamedKey::Hyper
                | NamedKey::Meta
                | NamedKey::CapsLock
                | NamedKey::NumLock
        )
    )
}

/// CSI 시퀀스의 키 번호부와 종결 문자.
struct SequenceBase {
    payload: String,
    terminator: char,
}

impl SequenceBase {
    fn new(payload: impl Into<String>, terminator: char) -> Self {
        Self {
            payload: payload.into(),
            terminator,
        }
    }
}

/// 시퀀스 수식자 비트 (kitty 규약: shift 1, alt 2, ctrl 4, super 8).
/// 전송값은 +1이다. winit ModifiersState에는 caps/num lock이 없어 보고하지
/// 않는다 — 스펙상 생략 허용.
#[derive(Clone, Copy)]
struct SeqMods(u8);

impl SeqMods {
    const SHIFT: u8 = 1;
    const ALT: u8 = 2;
    const CTRL: u8 = 4;
    const SUPER: u8 = 8;

    fn from_winit(mods: ModifiersState) -> Self {
        let mut v = 0;
        if mods.shift_key() {
            v |= Self::SHIFT;
        }
        if mods.alt_key() {
            v |= Self::ALT;
        }
        if mods.control_key() {
            v |= Self::CTRL;
        }
        if mods.super_key() {
            v |= Self::SUPER;
        }
        Self(v)
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }

    fn set(&mut self, bit: u8, on: bool) {
        if on {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }

    /// 시퀀스에 넣는 값 (비트합 + 1).
    fn encoded(self) -> u8 {
        self.0 + 1
    }
}

/// kitty 시퀀스를 조립한다. 표현할 수 없는 키는 None.
fn build_sequence(input: &KeyInput, mods: ModifiersState, mode: TermMode) -> Option<Vec<u8>> {
    let mut seq_mods = SeqMods::from_winit(mods);
    let encode_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    // 이벤트 종류 서브필드는 press(기본값 1)에서는 생략한다.
    let event_type = mode.contains(TermMode::REPORT_EVENT_TYPES)
        && (input.repeat || input.state == ElementState::Released);

    // REPORT_ASSOCIATED_TEXT: release에는 싣지 않고, 제어 문자도 싣지 않는다.
    let associated_text = input.text.as_deref().filter(|t| {
        mode.contains(TermMode::REPORT_ASSOCIATED_TEXT)
            && input.state != ElementState::Released
            && !t.is_empty()
            && !is_control_text(t)
    });

    // 판정 순서는 alacritty와 동일: numpad → PUA 기능키 → 레거시형 기능키 →
    // 제어·수식키 → 문자 키. `control_or_modifier_base`는 수식키 자신의 상태를
    // seq_mods에 반영하므로 가변 참조를 받는다.
    let base = if let Some(b) = numpad_base(input) {
        b
    } else if let Some(b) = named_kitty_base(input) {
        b
    } else if let Some(b) = named_normal_base(
        input,
        seq_mods.is_empty() && !event_type && associated_text.is_none(),
    ) {
        b
    } else if let Some(b) = control_or_modifier_base(input, encode_all, &mut seq_mods) {
        b
    } else {
        textual_base(input, seq_mods, mode, encode_all, associated_text)?
    };

    let mut out = format!("\x1b[{}", base.payload);
    // 수식자 필드는 기본값(1)이면서 이벤트·텍스트 필드도 없을 때만 생략한다.
    if event_type || !seq_mods.is_empty() || associated_text.is_some() {
        let _ = write!(out, ";{}", seq_mods.encoded());
    }
    if event_type {
        out.push(':');
        out.push(if input.repeat { '2' } else { '3' });
    }
    if let Some(text) = associated_text {
        let mut codepoints = text.chars().map(u32::from);
        if let Some(first) = codepoints.next() {
            let _ = write!(out, ";{first}");
        }
        for cp in codepoints {
            let _ = write!(out, ":{cp}");
        }
    }
    out.push(base.terminator);
    Some(out.into_bytes())
}

/// numpad 키 → 전용 코드포인트 (57399~). 메인 영역과 물리적으로 다른 키임을
/// 앱이 구분할 수 있어야 한다.
fn numpad_base(input: &KeyInput) -> Option<SequenceBase> {
    if input.location != KeyLocation::Numpad {
        return None;
    }
    let num = match input.logical_key.as_ref() {
        Key::Character("0") => "57399",
        Key::Character("1") => "57400",
        Key::Character("2") => "57401",
        Key::Character("3") => "57402",
        Key::Character("4") => "57403",
        Key::Character("5") => "57404",
        Key::Character("6") => "57405",
        Key::Character("7") => "57406",
        Key::Character("8") => "57407",
        Key::Character("9") => "57408",
        Key::Character(".") => "57409",
        Key::Character("/") => "57410",
        Key::Character("*") => "57411",
        Key::Character("-") => "57412",
        Key::Character("+") => "57413",
        Key::Character("=") => "57415",
        Key::Named(named) => match named {
            NamedKey::Enter => "57414",
            NamedKey::ArrowLeft => "57417",
            NamedKey::ArrowRight => "57418",
            NamedKey::ArrowUp => "57419",
            NamedKey::ArrowDown => "57420",
            NamedKey::PageUp => "57421",
            NamedKey::PageDown => "57422",
            NamedKey::Home => "57423",
            NamedKey::End => "57424",
            NamedKey::Insert => "57425",
            NamedKey::Delete => "57426",
            _ => return None,
        },
        _ => return None,
    };
    Some(SequenceBase::new(num, 'u'))
}

/// 레거시 표현이 없어 kitty가 사설 영역(PUA) 코드포인트를 정의한 기능키들.
/// F3만 예외적으로 `CSI 13~` — `CSI 1;mods R`이 커서 위치 보고(CPR)와
/// 충돌하기 때문에 kitty 스펙이 번호형으로 바꿨다.
fn named_kitty_base(input: &KeyInput) -> Option<SequenceBase> {
    let named = match input.logical_key {
        Key::Named(named) => named,
        _ => return None,
    };
    let (num, terminator) = match named {
        NamedKey::F3 => ("13", '~'),
        NamedKey::F13 => ("57376", 'u'),
        NamedKey::F14 => ("57377", 'u'),
        NamedKey::F15 => ("57378", 'u'),
        NamedKey::F16 => ("57379", 'u'),
        NamedKey::F17 => ("57380", 'u'),
        NamedKey::F18 => ("57381", 'u'),
        NamedKey::F19 => ("57382", 'u'),
        NamedKey::F20 => ("57383", 'u'),
        NamedKey::F21 => ("57384", 'u'),
        NamedKey::F22 => ("57385", 'u'),
        NamedKey::F23 => ("57386", 'u'),
        NamedKey::F24 => ("57387", 'u'),
        NamedKey::F25 => ("57388", 'u'),
        NamedKey::F26 => ("57389", 'u'),
        NamedKey::F27 => ("57390", 'u'),
        NamedKey::F28 => ("57391", 'u'),
        NamedKey::F29 => ("57392", 'u'),
        NamedKey::F30 => ("57393", 'u'),
        NamedKey::F31 => ("57394", 'u'),
        NamedKey::F32 => ("57395", 'u'),
        NamedKey::F33 => ("57396", 'u'),
        NamedKey::F34 => ("57397", 'u'),
        NamedKey::F35 => ("57398", 'u'),
        NamedKey::ScrollLock => ("57359", 'u'),
        NamedKey::PrintScreen => ("57361", 'u'),
        NamedKey::Pause => ("57362", 'u'),
        NamedKey::ContextMenu => ("57363", 'u'),
        NamedKey::MediaPlay => ("57428", 'u'),
        NamedKey::MediaPause => ("57429", 'u'),
        NamedKey::MediaPlayPause => ("57430", 'u'),
        NamedKey::MediaStop => ("57432", 'u'),
        NamedKey::MediaFastForward => ("57433", 'u'),
        NamedKey::MediaRewind => ("57434", 'u'),
        NamedKey::MediaTrackNext => ("57435", 'u'),
        NamedKey::MediaTrackPrevious => ("57436", 'u'),
        NamedKey::MediaRecord => ("57437", 'u'),
        NamedKey::AudioVolumeDown => ("57438", 'u'),
        NamedKey::AudioVolumeUp => ("57439", 'u'),
        NamedKey::AudioVolumeMute => ("57440", 'u'),
        _ => return None,
    };
    Some(SequenceBase::new(num, terminator))
}

/// 레거시 `CSI number ~` / `CSI 1 letter` 형을 유지하는 기능키. kitty 모드는
/// 이 형에 mods·이벤트 서브필드만 끼워 넣는다. `bare`(수식자·이벤트·텍스트
/// 전부 없음)이면 선두 "1"을 생략해 레거시와 같은 바이트가 된다 — 프로토콜을
/// 모르는 앱과의 호환이 여기서 나온다.
fn named_normal_base(input: &KeyInput, bare: bool) -> Option<SequenceBase> {
    let named = match input.logical_key {
        Key::Named(named) => named,
        _ => return None,
    };
    let one = if bare { "" } else { "1" };
    let (num, terminator) = match named {
        NamedKey::PageUp => ("5", '~'),
        NamedKey::PageDown => ("6", '~'),
        NamedKey::Insert => ("2", '~'),
        NamedKey::Delete => ("3", '~'),
        NamedKey::Home => (one, 'H'),
        NamedKey::End => (one, 'F'),
        NamedKey::ArrowLeft => (one, 'D'),
        NamedKey::ArrowRight => (one, 'C'),
        NamedKey::ArrowUp => (one, 'A'),
        NamedKey::ArrowDown => (one, 'B'),
        NamedKey::F1 => (one, 'P'),
        NamedKey::F2 => (one, 'Q'),
        // F3은 named_kitty_base에서 CSI 13~로 처리된다.
        NamedKey::F4 => (one, 'S'),
        NamedKey::F5 => ("15", '~'),
        NamedKey::F6 => ("17", '~'),
        NamedKey::F7 => ("18", '~'),
        NamedKey::F8 => ("19", '~'),
        NamedKey::F9 => ("20", '~'),
        NamedKey::F10 => ("21", '~'),
        NamedKey::F11 => ("23", '~'),
        NamedKey::F12 => ("24", '~'),
        _ => return None,
    };
    Some(SequenceBase::new(num, terminator))
}

/// 제어 문자 키(Enter·Esc 등)와, REPORT_ALL_KEYS_AS_ESC일 때의 수식키 자신.
///
/// winit은 수식자 상태 갱신(ModifiersChanged)을 수식키의 KeyboardInput보다
/// 늦게 보내므로, 수식키 자신의 이벤트에서는 그 키의 press 상태를 mods에
/// 직접 반영한다 — kitty 스펙이 요구하는 "press 후 상태"와 맞추기 위해서다.
fn control_or_modifier_base(
    input: &KeyInput,
    encode_all: bool,
    seq_mods: &mut SeqMods,
) -> Option<SequenceBase> {
    let named = match input.logical_key {
        Key::Named(named) => named,
        _ => return None,
    };

    let base = match named {
        NamedKey::Tab => "9",
        NamedKey::Enter => "13",
        NamedKey::Escape => "27",
        NamedKey::Space => "32",
        NamedKey::Backspace => "127",
        _ => "",
    };
    // 수식키·잠금키는 REPORT_ALL_KEYS_AS_ESC에서만 보고한다.
    if !encode_all && base.is_empty() {
        return None;
    }

    let base = match (named, input.location) {
        (NamedKey::Shift, KeyLocation::Left) => "57441",
        (NamedKey::Control, KeyLocation::Left) => "57442",
        (NamedKey::Alt, KeyLocation::Left) => "57443",
        (NamedKey::Super, KeyLocation::Left) => "57444",
        (NamedKey::Hyper, KeyLocation::Left) => "57445",
        (NamedKey::Meta, KeyLocation::Left) => "57446",
        (NamedKey::Shift, _) => "57447",
        (NamedKey::Control, _) => "57448",
        (NamedKey::Alt, _) => "57449",
        (NamedKey::Super, _) => "57450",
        (NamedKey::Hyper, _) => "57451",
        (NamedKey::Meta, _) => "57452",
        (NamedKey::CapsLock, _) => "57358",
        (NamedKey::NumLock, _) => "57360",
        _ => base,
    };

    let press = input.state == ElementState::Pressed;
    match named {
        NamedKey::Shift => seq_mods.set(SeqMods::SHIFT, press),
        NamedKey::Control => seq_mods.set(SeqMods::CTRL, press),
        NamedKey::Alt => seq_mods.set(SeqMods::ALT, press),
        NamedKey::Super => seq_mods.set(SeqMods::SUPER, press),
        _ => {}
    }

    if base.is_empty() {
        None
    } else {
        Some(SequenceBase::new(base, 'u'))
    }
}

/// 문자 키. unicode-key-code는 수식자를 뗀 코드포인트, shifted는
/// REPORT_ALTERNATE_KEYS일 때만 병기한다. 세 번째 서브필드(기본 배치 키)는
/// winit이 배치 정보를 노출하지 않아 생략한다 — 스펙상 허용 (alacritty 동일).
fn textual_base(
    input: &KeyInput,
    seq_mods: SeqMods,
    mode: TermMode,
    encode_all: bool,
    associated_text: Option<&str>,
) -> Option<SequenceBase> {
    let character = match input.logical_key.as_ref() {
        Key::Character(c) => c,
        _ => return None,
    };

    if character.chars().count() == 1 {
        let ch = character.chars().next().unwrap();
        let shift = seq_mods.shift();
        let unshifted = if shift {
            ch.to_lowercase().next().unwrap()
        } else {
            ch
        };

        let alternate_code = u32::from(ch);
        let mut key_code = u32::from(unshifted);
        // '1'→'!'처럼 대소문자 변환으로 복원되지 않는 시프트 문자는
        // key_without_modifiers에서 기본 키를 얻는다.
        if shift
            && alternate_code == key_code
            && let Key::Character(unmodded) = input.key_without_modifiers.as_ref()
        {
            key_code = unmodded.chars().next().map_or(key_code, u32::from);
        }

        let payload =
            if mode.contains(TermMode::REPORT_ALTERNATE_KEYS) && alternate_code != key_code {
                format!("{key_code}:{alternate_code}")
            } else {
                key_code.to_string()
            };
        Some(SequenceBase::new(payload, 'u'))
    } else if encode_all && associated_text.is_some() {
        // 키 하나로 대응이 안 되는 텍스트(다중 코드포인트)는 키 번호 0으로
        // 보내고 텍스트 필드에 싣는다 (스펙의 "pure text event").
        Some(SequenceBase::new("0", 'u'))
    } else {
        None
    }
}

/// 첫 코드포인트가 C0/DEL/C1 제어 문자인 한 글자 텍스트인가.
/// 이런 텍스트는 "키가 만든 텍스트"로 치지 않는다 (kitty·alacritty 동일).
fn is_control_text(text: &str) -> bool {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => {
            let cp = u32::from(c);
            cp < 0x20 || (0x7f..=0x9f).contains(&cp)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;
    const E: TermMode = TermMode::REPORT_EVENT_TYPES;
    const A: TermMode = TermMode::REPORT_ALTERNATE_KEYS;
    const K: TermMode = TermMode::REPORT_ALL_KEYS_AS_ESC;
    const T: TermMode = TermMode::REPORT_ASSOCIATED_TEXT;

    const NONE: ModifiersState = ModifiersState::empty();
    const SHIFT: ModifiersState = ModifiersState::SHIFT;
    const ALT: ModifiersState = ModifiersState::ALT;
    const CTRL: ModifiersState = ModifiersState::CONTROL;

    /// 문자 키 press. `key`는 logical(시프트 반영), `unmod`는 수식자 뗀 키.
    fn chr(key: &str, unmod: &str, text: Option<&str>) -> KeyInput {
        KeyInput {
            logical_key: Key::Character(key.into()),
            key_without_modifiers: Key::Character(unmod.into()),
            location: KeyLocation::Standard,
            state: ElementState::Pressed,
            repeat: false,
            text: text.map(str::to_owned),
        }
    }

    /// 이름 있는 키 press. 텍스트는 winit `to_text()`와 같은 값이 들어온다.
    fn named(key: NamedKey) -> KeyInput {
        KeyInput {
            logical_key: Key::Named(key),
            key_without_modifiers: Key::Named(key),
            location: KeyLocation::Standard,
            state: ElementState::Pressed,
            repeat: false,
            text: Key::Named(key).to_text().map(str::to_owned),
        }
    }

    fn released(mut input: KeyInput) -> KeyInput {
        input.state = ElementState::Released;
        input.text = None; // release에는 텍스트가 없다
        input
    }

    fn repeated(mut input: KeyInput) -> KeyInput {
        input.repeat = true;
        input
    }

    fn numpad(mut input: KeyInput) -> KeyInput {
        input.location = KeyLocation::Numpad;
        input
    }

    fn left(mut input: KeyInput) -> KeyInput {
        input.location = KeyLocation::Left;
        input
    }

    fn seq(s: &str) -> Encoded {
        Encoded::Bytes(s.as_bytes().to_vec())
    }

    // ─── 디스패치: 어떤 키가 레거시로 남는가 ───

    #[test]
    fn no_kitty_flags_means_legacy() {
        let a = chr("a", "a", Some("a"));
        assert_eq!(encode(&a, NONE, TermMode::empty()), Encoded::Legacy);
        assert_eq!(
            encode(&released(chr("a", "a", Some("a"))), NONE, TermMode::empty()),
            Encoded::Nothing,
            "kitty가 꺼져 있으면 release는 버린다 (기존 동작)"
        );
    }

    #[test]
    fn alternate_and_text_flags_alone_do_not_change_dispatch() {
        // A·T는 다른 플래그가 만든 시퀀스를 장식할 뿐이다.
        let ctrl_a = chr("a", "a", Some("\x01"));
        assert_eq!(encode(&ctrl_a, CTRL, A | T), Encoded::Legacy);
    }

    #[test]
    fn disambiguate_keeps_plain_and_shifted_text_legacy() {
        assert_eq!(encode(&chr("a", "a", Some("a")), NONE, D), Encoded::Legacy);
        assert_eq!(
            encode(&chr("A", "a", Some("A")), SHIFT, D),
            Encoded::Legacy,
            "Shift만 붙은 문자는 여전히 평문"
        );
    }

    #[test]
    fn disambiguate_keeps_unmodified_enter_tab_backspace_legacy() {
        assert_eq!(encode(&named(NamedKey::Enter), NONE, D), Encoded::Legacy);
        assert_eq!(encode(&named(NamedKey::Tab), NONE, D), Encoded::Legacy);
        assert_eq!(
            encode(&named(NamedKey::Backspace), NONE, D),
            Encoded::Legacy,
            "스펙의 예외 — 모드가 남아 죽어도 셸에서 reset을 칠 수 있어야 한다"
        );
        assert_eq!(encode(&named(NamedKey::Space), NONE, D), Encoded::Legacy);
    }

    // ─── DISAMBIGUATE_ESC_CODES ───

    #[test]
    fn esc_is_disambiguated() {
        assert_eq!(encode(&named(NamedKey::Escape), NONE, D), seq("\x1b[27u"));
        assert_eq!(encode(&named(NamedKey::Escape), CTRL, D), seq("\x1b[27;5u"));
    }

    #[test]
    fn ctrl_letter_becomes_csi_u() {
        let ctrl_a = chr("a", "a", Some("\x01"));
        assert_eq!(encode(&ctrl_a, CTRL, D), seq("\x1b[97;5u"));
    }

    #[test]
    fn ctrl_shift_letter_reports_the_unshifted_code() {
        // Ctrl+Shift+A: 키 코드는 언시프트('a'=97), mods는 shift+ctrl=6.
        let input = chr("A", "a", Some("\x01"));
        assert_eq!(encode(&input, SHIFT | CTRL, D), seq("\x1b[97;6u"));
    }

    #[test]
    fn ctrl_i_is_distinct_from_tab() {
        // 레거시에서 Ctrl+I와 Tab은 똑같이 \t였다 — 이 구분이 이 Phase의 목적.
        let ctrl_i = chr("i", "i", Some("\t"));
        assert_eq!(encode(&ctrl_i, CTRL, D), seq("\x1b[105;5u"));
        assert_eq!(encode(&named(NamedKey::Tab), NONE, D), Encoded::Legacy);
    }

    #[test]
    fn modified_enter_tab_backspace_become_csi_u() {
        // kitty와 같고 alacritty와 다른 지점: 수식자가 붙는 순간 CSI u다.
        assert_eq!(encode(&named(NamedKey::Enter), SHIFT, D), seq("\x1b[13;2u"));
        assert_eq!(
            encode(&named(NamedKey::Tab), SHIFT, D),
            seq("\x1b[9;2u"),
            "Shift+Tab은 레거시 \\x1b[Z가 아니라 CSI u"
        );
        assert_eq!(encode(&named(NamedKey::Enter), CTRL, D), seq("\x1b[13;5u"));
        assert_eq!(
            encode(&named(NamedKey::Backspace), CTRL, D),
            seq("\x1b[127;5u")
        );
    }

    #[test]
    fn ctrl_space_is_csi_u() {
        let mut space = named(NamedKey::Space);
        space.text = Some("\x00".into());
        assert_eq!(encode(&space, CTRL, D), seq("\x1b[32;5u"));
    }

    // ─── 화살표·기능키 ───

    #[test]
    fn bare_arrows_match_legacy_bytes() {
        assert_eq!(encode(&named(NamedKey::ArrowUp), NONE, D), seq("\x1b[A"));
        assert_eq!(encode(&named(NamedKey::Home), NONE, D), seq("\x1b[H"));
        assert_eq!(encode(&named(NamedKey::End), NONE, D), seq("\x1b[F"));
    }

    #[test]
    fn modified_arrows_gain_the_mods_field() {
        assert_eq!(
            encode(&named(NamedKey::ArrowLeft), CTRL, D),
            seq("\x1b[1;5D")
        );
        assert_eq!(encode(&named(NamedKey::ArrowUp), ALT, D), seq("\x1b[1;3A"));
        assert_eq!(
            encode(&named(NamedKey::ArrowUp), SHIFT | ALT | CTRL, D),
            seq("\x1b[1;8A"),
            "1+2+4 +1 = 8"
        );
    }

    #[test]
    fn function_keys_f1_to_f12() {
        assert_eq!(encode(&named(NamedKey::F1), NONE, D), seq("\x1b[P"));
        assert_eq!(encode(&named(NamedKey::F2), NONE, D), seq("\x1b[Q"));
        assert_eq!(
            encode(&named(NamedKey::F3), NONE, D),
            seq("\x1b[13~"),
            "F3은 CSI R이 커서 위치 보고와 충돌해 번호형"
        );
        assert_eq!(encode(&named(NamedKey::F4), NONE, D), seq("\x1b[S"));
        assert_eq!(encode(&named(NamedKey::F5), NONE, D), seq("\x1b[15~"));
        assert_eq!(encode(&named(NamedKey::F6), NONE, D), seq("\x1b[17~"));
        assert_eq!(encode(&named(NamedKey::F7), NONE, D), seq("\x1b[18~"));
        assert_eq!(encode(&named(NamedKey::F8), NONE, D), seq("\x1b[19~"));
        assert_eq!(encode(&named(NamedKey::F9), NONE, D), seq("\x1b[20~"));
        assert_eq!(encode(&named(NamedKey::F10), NONE, D), seq("\x1b[21~"));
        assert_eq!(encode(&named(NamedKey::F11), NONE, D), seq("\x1b[23~"));
        assert_eq!(encode(&named(NamedKey::F12), NONE, D), seq("\x1b[24~"));
    }

    #[test]
    fn modified_function_keys() {
        assert_eq!(encode(&named(NamedKey::F1), SHIFT, D), seq("\x1b[1;2P"));
        assert_eq!(encode(&named(NamedKey::F3), SHIFT, D), seq("\x1b[13;2~"));
        assert_eq!(encode(&named(NamedKey::F12), CTRL, D), seq("\x1b[24;5~"));
    }

    #[test]
    fn tilde_keys_keep_their_numbers() {
        assert_eq!(encode(&named(NamedKey::Delete), NONE, D), seq("\x1b[3~"));
        assert_eq!(encode(&named(NamedKey::PageUp), NONE, D), seq("\x1b[5~"));
        assert_eq!(
            encode(&named(NamedKey::PageDown), CTRL, D),
            seq("\x1b[6;5~")
        );
        assert_eq!(encode(&named(NamedKey::Insert), NONE, D), seq("\x1b[2~"));
    }

    // ─── REPORT_EVENT_TYPES ───

    #[test]
    fn release_needs_the_event_types_flag() {
        let up = released(named(NamedKey::ArrowUp));
        assert_eq!(encode(&up, NONE, D), Encoded::Nothing, "E 없으면 버린다");
        assert_eq!(encode(&up, NONE, D | E), seq("\x1b[1;1:3A"));
    }

    #[test]
    fn repeat_and_release_subfields() {
        assert_eq!(
            encode(&repeated(named(NamedKey::ArrowUp)), NONE, D | E),
            seq("\x1b[1;1:2A")
        );
        assert_eq!(
            encode(&released(named(NamedKey::ArrowUp)), SHIFT, D | E),
            seq("\x1b[1;2:3A")
        );
        assert_eq!(
            encode(&released(named(NamedKey::Escape)), NONE, D | E),
            seq("\x1b[27;1:3u")
        );
    }

    #[test]
    fn text_keys_report_release_but_press_stays_text() {
        // press는 평문 "a", release만 CSI — kitty의 실제 동작.
        assert_eq!(
            encode(&chr("a", "a", Some("a")), NONE, D | E),
            Encoded::Legacy
        );
        assert_eq!(
            encode(&released(chr("a", "a", None)), NONE, D | E),
            seq("\x1b[97;1:3u")
        );
        // repeat는 press와 같이 평문.
        assert_eq!(
            encode(&repeated(chr("a", "a", Some("a"))), NONE, D | E),
            Encoded::Legacy
        );
    }

    #[test]
    fn enter_tab_backspace_have_no_release_without_encode_all() {
        assert_eq!(
            encode(&released(named(NamedKey::Enter)), NONE, D | E),
            Encoded::Nothing
        );
        assert_eq!(
            encode(&released(named(NamedKey::Tab)), NONE, D | E),
            Encoded::Nothing
        );
        assert_eq!(
            encode(&released(named(NamedKey::Backspace)), NONE, D | E),
            Encoded::Nothing
        );
        // REPORT_ALL_KEYS_AS_ESC이면 release도 보고한다.
        assert_eq!(
            encode(&released(named(NamedKey::Enter)), NONE, K | E),
            seq("\x1b[13;1:3u")
        );
    }

    #[test]
    fn event_types_alone_keeps_esc_press_legacy() {
        // E만 켠 앱에서 Esc press는 레거시 \x1b 그대로 (kitty 동일).
        assert_eq!(encode(&named(NamedKey::Escape), NONE, E), Encoded::Legacy);
        assert_eq!(
            encode(&released(named(NamedKey::Escape)), NONE, E),
            seq("\x1b[27;1:3u")
        );
    }

    // ─── REPORT_ALTERNATE_KEYS ───

    #[test]
    fn alternate_keys_add_the_shifted_codepoint() {
        let input = chr("A", "a", Some("\x01"));
        assert_eq!(encode(&input, SHIFT | CTRL, D | A), seq("\x1b[97:65;6u"));
    }

    #[test]
    fn alternate_keys_use_key_without_modifiers_for_symbols() {
        // Shift+1 = '!': 소문자 변환으로는 기본 키를 못 얻는다.
        let input = chr("!", "1", Some("\x01"));
        assert_eq!(encode(&input, SHIFT | CTRL, D | A), seq("\x1b[49:33;6u"));
    }

    #[test]
    fn alternate_keys_omitted_without_shift() {
        let ctrl_a = chr("a", "a", Some("\x01"));
        assert_eq!(
            encode(&ctrl_a, CTRL, D | A),
            seq("\x1b[97;5u"),
            "shift가 없으면 shifted 서브필드도 없다"
        );
    }

    // ─── REPORT_ALL_KEYS_AS_ESC ───

    #[test]
    fn encode_all_turns_plain_text_keys_into_csi_u() {
        assert_eq!(encode(&chr("a", "a", Some("a")), NONE, K), seq("\x1b[97u"));
        assert_eq!(
            encode(&chr("A", "a", Some("A")), SHIFT, K),
            seq("\x1b[97;2u")
        );
    }

    #[test]
    fn encode_all_covers_enter_tab_backspace_escape() {
        assert_eq!(encode(&named(NamedKey::Enter), NONE, K), seq("\x1b[13u"));
        assert_eq!(encode(&named(NamedKey::Tab), NONE, K), seq("\x1b[9u"));
        assert_eq!(
            encode(&named(NamedKey::Backspace), NONE, K),
            seq("\x1b[127u")
        );
        assert_eq!(encode(&named(NamedKey::Escape), NONE, K), seq("\x1b[27u"));
        assert_eq!(encode(&named(NamedKey::Space), NONE, K), seq("\x1b[32u"));
    }

    #[test]
    fn modifier_keys_are_reported_only_with_encode_all() {
        assert_eq!(
            encode(&left(named(NamedKey::Shift)), SHIFT, D),
            Encoded::Nothing,
            "REPORT_ALL 없이 수식키 단독은 무보고"
        );
        assert_eq!(
            encode(&left(named(NamedKey::Shift)), SHIFT, K),
            seq("\x1b[57441;2u"),
            "자기 자신이 mods에 들어간다"
        );
        assert_eq!(
            encode(&named(NamedKey::Shift), SHIFT, K),
            seq("\x1b[57447;2u"),
            "오른쪽(비-Left) Shift"
        );
        assert_eq!(
            encode(&left(named(NamedKey::Super)), ModifiersState::SUPER, K),
            seq("\x1b[57444;9u")
        );
    }

    #[test]
    fn modifier_release_clears_its_own_bit() {
        // winit은 수식자 상태를 키 이벤트보다 늦게 갱신하므로 키 자신의 상태로
        // 보정한다: release 시점의 mods에 아직 shift가 남아 있어도 1:3이어야 한다.
        assert_eq!(
            encode(&released(left(named(NamedKey::Shift))), SHIFT, K | E),
            seq("\x1b[57441;1:3u")
        );
    }

    #[test]
    fn caps_lock_uses_its_dedicated_codepoint() {
        assert_eq!(
            encode(&named(NamedKey::CapsLock), NONE, K),
            seq("\x1b[57358u")
        );
        assert_eq!(
            encode(&named(NamedKey::CapsLock), NONE, D),
            Encoded::Nothing
        );
    }

    #[test]
    fn encode_all_reports_repeat_of_text_keys() {
        assert_eq!(
            encode(&repeated(chr("a", "a", Some("a"))), NONE, K | E),
            seq("\x1b[97;1:2u")
        );
    }

    // ─── REPORT_ASSOCIATED_TEXT ───

    #[test]
    fn associated_text_carries_codepoints() {
        assert_eq!(
            encode(&chr("a", "a", Some("a")), NONE, K | T),
            seq("\x1b[97;1;97u"),
            "텍스트 필드가 있으면 mods 필드(기본 1)를 생략할 수 없다"
        );
        assert_eq!(
            encode(&chr("A", "a", Some("A")), SHIFT, K | T),
            seq("\x1b[97;2;65u")
        );
    }

    #[test]
    fn associated_text_skips_control_characters() {
        let ctrl_a = chr("a", "a", Some("\x01"));
        assert_eq!(
            encode(&ctrl_a, CTRL, D | T),
            seq("\x1b[97;5u"),
            "제어 문자는 텍스트로 싣지 않는다"
        );
    }

    #[test]
    fn associated_text_never_rides_on_release() {
        assert_eq!(
            encode(&released(chr("a", "a", None)), NONE, K | E | T),
            seq("\x1b[97;1:3u")
        );
    }

    #[test]
    fn multi_codepoint_text_uses_key_number_zero() {
        // 키 하나로 대응 안 되는 텍스트 — 스펙의 "pure text event".
        let input = chr("ab", "ab", Some("ab"));
        assert_eq!(encode(&input, NONE, K | T), seq("\x1b[0;1;97:98u"));
    }

    #[test]
    fn multibyte_text_is_encoded_as_codepoints() {
        let input = chr("ㅎ", "ㅎ", Some("ㅎ"));
        assert_eq!(
            encode(&input, NONE, K | T),
            seq("\x1b[12622;1;12622u"),
            "코드포인트는 10진수 유니코드 스칼라"
        );
    }

    // ─── numpad ───

    #[test]
    fn numpad_keys_use_dedicated_codepoints_from_disambiguate_on() {
        assert_eq!(
            encode(&numpad(chr("5", "5", Some("5"))), NONE, D),
            seq("\x1b[57404u"),
            "본체 '5'와 구분돼야 한다 — 평문으로 새면 안 된다"
        );
        assert_eq!(
            encode(&numpad(named(NamedKey::Enter)), NONE, D),
            seq("\x1b[57414u")
        );
        assert_eq!(
            encode(&numpad(chr("+", "+", Some("+"))), CTRL, D),
            seq("\x1b[57413;5u")
        );
    }

    // ─── macOS Option(Alt) 처리 ───

    #[test]
    fn option_composed_characters_stay_legacy_text() {
        // Option+a = å: macOS에서 Option은 조합 키다. ALT를 수식자로 보고하면
        // 조합 문자가 죽는다.
        let input = chr("å", "a", Some("å"));
        assert_eq!(encode(&input, ALT, D), Encoded::Legacy);
    }

    #[test]
    fn alt_stays_a_modifier_for_non_text_keys() {
        assert_eq!(encode(&named(NamedKey::ArrowUp), ALT, D), seq("\x1b[1;3A"));
        // Ctrl+Alt+문자: 텍스트가 제어 문자라 조합이 아니다 — ALT 유지.
        let input = chr("a", "a", Some("\x01"));
        assert_eq!(encode(&input, CTRL | ALT, D), seq("\x1b[97;7u"));
    }
}
