//! 명령 완료 알림 + OSC 9/777 (Phase 17).
//!
//! 판단(순수 함수)과 전달(AppKit)을 분리한다. reader 스레드는 사실만 보내고
//! (`AppEvent::CommandFinished` / `AppEvent::Notify`), 조건 판단과 AppKit
//! 호출은 전부 메인 스레드(App)에서 한다 — AppKit은 메인 스레드 밖에서
//! 부르면 안 된다. 조건 함수들은 mouse_report.rs처럼 GPU·PTY 없이
//! 단위 테스트로 덮는다.

use std::time::Duration;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};

use super::App;

/// 명령 완료 알림을 낼지 판단한다.
///
/// "보고 있지 않다"의 정의: 창이 비포커스이거나, 포커스라도 해당 페인의
/// 탭이 비활성(다른 탭을 보는 중). 임계값 미만의 짧은 명령은 거른다.
pub(super) fn should_notify_command(
    enabled: bool,
    threshold: Duration,
    duration: Duration,
    window_focused: bool,
    tab_active: bool,
) -> bool {
    enabled && duration >= threshold && !(window_focused && tab_active)
}

/// OSC 9/777 알림을 낼지 판단한다. 소요 시간 조건이 없다는 점만 다르다 —
/// 대신 보고 있는 페인의 알림을 걸러 스팸을 막는다 (포커스 중 무시).
pub(super) fn should_notify_osc(enabled: bool, window_focused: bool, tab_active: bool) -> bool {
    enabled && !(window_focused && tab_active)
}

/// 알림에 쓰는 소요 시간 표기: "32초" / "2분 5초".
pub(super) fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}분 {}초", secs / 60, secs % 60)
    } else {
        format!("{secs}초")
    }
}

/// 명령 완료 알림의 (제목, 본문).
///
/// 본문은 프롬프트 줄(명령 텍스트 포함)이고, 뽑지 못했으면 빈 문자열 —
/// 제목만으로도 무슨 일이 있었는지 알 수 있게 결과·소요 시간을 제목에 넣는다.
/// exit 코드를 모르는 경우(None)는 실패라고 단정할 수 없어 "완료"로 둔다.
pub(super) fn command_message(
    exit: Option<i32>,
    duration: Duration,
    prompt: Option<String>,
) -> (String, String) {
    let took = format_duration(duration);
    let title = match exit {
        Some(code) if code != 0 => format!("명령 실패 (exit {code}, {took})"),
        _ => format!("명령 완료 ({took})"),
    };
    (title, prompt.unwrap_or_default())
}

/// macOS 알림 전달: ① Dock 바운스 ② NSUserNotification 배너.
///
/// requestUserAttention은 권한·번들 조건이 없고, 앱이 이미 전면이면
/// 시스템이 무시하므로 이중 안전장치가 된다.
pub(super) fn deliver(title: &str, body: &str) {
    use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
    use objc2_foundation::MainThreadMarker;

    // App 이벤트 경로는 항상 메인 스레드지만, 잘못 옮겨 쓰면 크래시가 아니라
    // 조용히 무시되도록 방어한다.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let _ = NSApplication::sharedApplication(mtm)
        .requestUserAttention(NSRequestUserAttentionType::NSInformationalRequest);

    deliver_banner(title, body);
}

/// NSUserNotification 배너.
///
/// 10.14부터 deprecated지만 일부러 쓴다 — 신식 UNUserNotificationCenter는
/// 번들 밖(`cargo run`) 실행에서 크래시한다.
///
/// **번들 밖에서는 `defaultUserNotificationCenter`가 nil을 돌려준다.** 알림
/// 센터는 번들 식별자로 앱을 구분하는데 맨 바이너리에는 그게 없기 때문이다.
/// objc2가 생성한 바인딩은 반환값을 non-null로 선언해 nil이 오면 패닉하므로
/// (실측: `cargo run`에서 알림이 뜰 때마다 앱이 죽었다), 여기서는 바인딩을
/// 우회해 직접 메시지를 보내고 Option으로 받는다. nil이면 배너를 포기하고
/// Dock 바운스만 남긴다 — 개발 중 실행에서도 앱은 살아 있어야 한다.
#[allow(deprecated)]
fn deliver_banner(title: &str, body: &str) {
    use objc2::rc::Retained;
    use objc2::{class, msg_send_id};
    use objc2_foundation::{NSString, NSUserNotification, NSUserNotificationCenter};

    unsafe {
        let center: Option<Retained<NSUserNotificationCenter>> = msg_send_id![
            class!(NSUserNotificationCenter),
            defaultUserNotificationCenter
        ];
        let Some(center) = center else {
            return; // 번들 밖 실행 — 배너 없음, Dock 바운스로 충분
        };
        let notification = NSUserNotification::new();
        notification.setTitle(Some(&NSString::from_str(title)));
        if !body.is_empty() {
            notification.setInformativeText(Some(&NSString::from_str(body)));
        }
        center.deliverNotification(&notification);
    }
}

/// 방금 끝난 명령 블록의 프롬프트 줄(프롬프트 + 명령 텍스트)을 뽑는다.
///
/// 명령 텍스트를 따로 저장하지 않으므로 화면에서 읽는다 — copy_last_output과
/// 같은 bounds_to_string 관용구. 여러 줄 프롬프트(p10k 등)를 위해 A 마크부터
/// C 마크 직전 줄까지 뽑는다. 스크롤백 밖으로 밀려났으면 None — 알림은
/// 제목만으로 폴백한다.
fn prompt_line(session: &crate::session::Session) -> Option<String> {
    let block = session
        .blocks()
        .into_iter()
        .rev()
        .find(|b| b.end_abs.is_some())?;
    let start_abs = block.start_abs;
    let end_abs = block
        .cmd_abs
        .map(|c| c - 1)
        .unwrap_or(start_abs)
        .max(start_abs);

    let text = {
        let term = session.term.lock();
        let history = term.grid().history_size() as i64;
        let cols = term.grid().columns();
        let screen = Dimensions::screen_lines(term.grid()) as i64;
        let start_line = start_abs - history;
        let end_line = end_abs - history;
        // 범위 밖 Point로 bounds_to_string을 부르면 패닉할 수 있다.
        if start_line < -history || end_line >= screen {
            return None;
        }
        let start = Point::new(Line(start_line as i32), Column(0));
        let end = Point::new(Line(end_line as i32), Column(cols - 1));
        term.bounds_to_string(start, end)
    };
    // 알림 한 줄에 들어가도록 개행·연속 공백을 접고 길이를 자른다.
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    Some(truncate_chars(text, 120))
}

/// 문자 수 기준으로 자른다 (바이트로 자르면 한글 경계에서 패닉).
fn truncate_chars(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

impl App {
    /// OSC 133 D — 명령 하나가 끝났다 (reader 스레드가 보낸 이벤트).
    pub(super) fn on_command_finished(
        &mut self,
        pane_id: usize,
        duration: Duration,
        exit: Option<i32>,
    ) {
        let Some(state) = &self.state else { return };
        let Some(tab_index) = state
            .tabs
            .iter()
            .position(|t| t.root.pane(pane_id).is_some())
        else {
            return; // 페인이 이미 닫혔다
        };
        if !should_notify_command(
            self.config.notify,
            Duration::from_secs(self.config.notify_threshold),
            duration,
            self.window_focused,
            tab_index == state.active,
        ) {
            return;
        }
        let prompt = state.tabs[tab_index]
            .root
            .pane(pane_id)
            .and_then(|p| prompt_line(&p.session));
        let (title, body) = command_message(exit, duration, prompt);
        deliver(&title, &body);
    }

    /// OSC 9/777 알림 (reader 스레드가 보낸 이벤트).
    pub(super) fn on_osc_notify(&mut self, pane_id: usize, title: Option<String>, body: String) {
        let Some(state) = &self.state else { return };
        let Some(tab_index) = state
            .tabs
            .iter()
            .position(|t| t.root.pane(pane_id).is_some())
        else {
            return;
        };
        if !should_notify_osc(
            self.config.notify,
            self.window_focused,
            tab_index == state.active,
        ) {
            return;
        }
        // OSC 9는 제목이 없다 — 앱 이름을 제목으로 쓴다 (iTerm2와 동일한 관례).
        let title = title.unwrap_or_else(|| "eden".to_string());
        deliver(&title, &body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T10: Duration = Duration::from_secs(10);

    #[test]
    fn command_notification_requires_threshold_and_unfocus() {
        // 정상 케이스: 비포커스 + 임계값 이상
        assert!(should_notify_command(true, T10, T10, false, true));

        // 임계값 미만은 거른다 (경계값 포함 여부: 이상이면 알림)
        assert!(!should_notify_command(
            true,
            T10,
            Duration::from_secs(9),
            false,
            true
        ));

        // 포커스 + 활성 탭 = 보고 있다 → 알림 없음
        assert!(!should_notify_command(true, T10, T10, true, true));

        // 포커스라도 비활성 탭이면 알림 (다른 탭을 보는 중)
        assert!(should_notify_command(true, T10, T10, true, false));

        // 설정 off면 무조건 없음
        assert!(!should_notify_command(false, T10, T10, false, false));
    }

    #[test]
    fn osc_notification_only_checks_focus() {
        assert!(should_notify_osc(true, false, true), "비포커스");
        assert!(should_notify_osc(true, true, false), "비활성 탭");
        assert!(
            !should_notify_osc(true, true, true),
            "보고 있으면 스팸 방지"
        );
        assert!(!should_notify_osc(false, false, false), "설정 off");
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(Duration::from_secs(32)), "32초");
        assert_eq!(format_duration(Duration::from_secs(60)), "1분 0초");
        assert_eq!(format_duration(Duration::from_secs(125)), "2분 5초");
        assert_eq!(
            format_duration(Duration::from_millis(10_900)),
            "10초",
            "초 미만은 버림"
        );
    }

    #[test]
    fn command_message_encodes_result_in_title() {
        let (title, body) = command_message(
            Some(0),
            Duration::from_secs(32),
            Some("~ ❯ cargo build".to_string()),
        );
        assert_eq!(title, "명령 완료 (32초)");
        assert_eq!(body, "~ ❯ cargo build");

        let (title, body) = command_message(Some(2), Duration::from_secs(75), None);
        assert_eq!(title, "명령 실패 (exit 2, 1분 15초)");
        assert_eq!(body, "", "프롬프트 추출 실패 시 제목만으로 폴백");

        // exit 코드를 모르면(D에 코드가 없던 경우) 실패로 단정하지 않는다.
        let (title, _) = command_message(None, Duration::from_secs(11), None);
        assert_eq!(title, "명령 완료 (11초)");
    }

    #[test]
    fn truncation_is_char_boundary_safe() {
        assert_eq!(truncate_chars("짧다".to_string(), 120), "짧다");
        let long = "가".repeat(130);
        let cut = truncate_chars(long, 120);
        assert_eq!(cut.chars().count(), 121, "120자 + 말줄임표");
        assert!(cut.ends_with('…'));
    }
}
