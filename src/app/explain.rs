//! AI 에러 설명 (Cmd+Shift+K) — 마지막으로 실패한 명령을 스트리밍으로 설명한다.
//!
//! Cmd+K(명령 생성)와 같은 로컬 우선 라우팅(BYOK → Ollama)을 쓰고, 같은 안전
//! 원칙을 따른다: 설명 끝의 `FIX:` 명령은 Enter로 **입력줄에 넣기만** 한다.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};

use super::App;
use super::ai_bar::truncate_tail;
use crate::ai;
use crate::session::AppEvent;

/// 설명에 보낼 출력의 최대 바이트 (뒤쪽 — 에러는 대개 끝에 있다).
const OUTPUT_MAX_BYTES: usize = 8000;

pub(super) struct Explain {
    pub(super) seq: u64,
    title: String,
    text: String,
    status: Status,
}

enum Status {
    Streaming,
    Done,
    Failed(String),
}

impl Explain {
    /// 패널에 그릴 (제목, 본문, 하단 안내).
    pub(super) fn panel(&self) -> (String, String, String) {
        let body = match &self.status {
            Status::Failed(err) if self.text.is_empty() => format!("오류: {err}"),
            Status::Failed(err) => format!("{}\n\n(중단됨: {err})", self.text),
            _ if self.text.is_empty() => "분석 중…".to_string(),
            _ => self.text.clone(),
        };
        let footer = match &self.status {
            Status::Streaming => "Esc 닫기",
            Status::Done if ai::fix_command(&self.text).is_some() => {
                "Enter: FIX 명령을 입력줄에 넣기 (실행하지 않음) · Esc 닫기"
            }
            _ => "Enter / Esc 닫기",
        };
        (self.title.clone(), body, footer.to_string())
    }
}

impl App {
    /// 포커스된 페인의 마지막 실패 명령을 설명한다. 실패한 명령이 없으면 그렇게 알린다.
    pub(super) fn explain_last_failure(&mut self) {
        let state = self.state.as_ref().unwrap();
        let pane = state.focused_pane();
        self.explain_seq += 1;
        let seq = self.explain_seq;

        let Some(failure) = pane.session.last_failure() else {
            self.explain = Some(Explain {
                seq,
                title: "AI 설명".to_string(),
                text: String::new(),
                status: Status::Failed(
                    "이 탭에 실패한 명령이 없습니다 (셸 통합이 켜져 있어야 기록됩니다)".to_string(),
                ),
            });
            state.window.request_redraw();
            return;
        };

        let output = {
            let term = pane.session.term.lock();
            let grid = term.grid();
            let history = grid.history_size() as i64;
            let top = -(history as i32);
            // 절대 줄 → 그리드 줄. 스크롤백 상한 밖으로 밀려난 앞부분은 버린다.
            let start = ((failure.output_start - history) as i32).max(top);
            let end = (failure.output_end - history) as i32;
            if end < start {
                String::new()
            } else {
                let text = term.bounds_to_string(
                    Point::new(Line(start), Column(0)),
                    Point::new(Line(end), Column(grid.columns() - 1)),
                );
                truncate_tail(text, OUTPUT_MAX_BYTES)
            }
        };

        let label = failure.command.as_deref().unwrap_or("(명령 알 수 없음)");
        let exit = failure
            .exit
            .map_or_else(|| "?".to_string(), |e| e.to_string());
        self.explain = Some(Explain {
            seq,
            title: format!("AI 설명 — {label}  (exit {exit})"),
            text: String::new(),
            status: Status::Streaming,
        });

        let context = ai::ExplainContext {
            command: failure.command,
            exit: failure.exit,
            output,
            cwd: pane.session.cwd(),
        };
        let proxy = self.proxy.clone();
        let settings = self.ai_settings();
        std::thread::spawn(move || {
            let mut on_text = |text: &str| {
                let _ = proxy.send_event(AppEvent::AiExplainDelta {
                    seq,
                    text: text.to_string(),
                });
            };
            let result = ai::explain_failure(&context, &settings, &mut on_text);
            let _ = proxy.send_event(AppEvent::AiExplainDone {
                seq,
                error: result.err(),
            });
        });
        state.window.request_redraw();
    }

    pub(super) fn on_explain_delta(&mut self, seq: u64, text: String) {
        if let Some(explain) = &mut self.explain
            && explain.seq == seq
        {
            explain.text.push_str(&text);
            self.state.as_ref().unwrap().window.request_redraw();
        }
    }

    pub(super) fn on_explain_done(&mut self, seq: u64, error: Option<String>) {
        if let Some(explain) = &mut self.explain
            && explain.seq == seq
        {
            explain.status = match error {
                Some(err) => Status::Failed(err),
                None => Status::Done,
            };
            self.state.as_ref().unwrap().window.request_redraw();
        }
    }

    /// 설명 패널이 열려 있을 때의 키 처리. 열려 있으면 키를 전부 삼킨다
    /// (설명을 읽는 동안 셸에 글자가 새지 않게).
    pub(super) fn explain_key(&mut self, key: &winit::keyboard::Key) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let Some(explain) = &self.explain else {
            return false;
        };
        match key {
            Key::Named(NamedKey::Escape) => {
                self.explain = None;
                self.explain_seq += 1; // 진행 중인 스트림의 남은 조각은 버린다
            }
            Key::Named(NamedKey::Enter) => {
                if matches!(explain.status, Status::Streaming) {
                    return true; // 다 받기 전에는 FIX 줄이 아직 없다
                }
                let fix = ai::fix_command(&explain.text);
                self.explain = None;
                if let Some(cmd) = fix {
                    self.insert_command(&cmd);
                }
            }
            _ => {}
        }
        self.state.as_ref().unwrap().window.request_redraw();
        true
    }
}
