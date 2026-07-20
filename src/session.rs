//! 터미널 세션: PTY 위에 셸을 띄우고 alacritty_terminal의 그리드 상태를 유지한다.

use std::borrow::Cow;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop as PtyEventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use winit::event_loop::EventLoopProxy;

/// 터미널 이벤트를 winit 이벤트 루프로 전달하는 리스너.
#[derive(Clone)]
pub struct EventProxy(EventLoopProxy<Event>);

impl EventProxy {
    pub fn new(proxy: EventLoopProxy<Event>) -> Self {
        Self(proxy)
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send_event(event);
    }
}

pub struct TermSize {
    pub columns: usize,
    pub lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

pub struct Session {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    notifier: Notifier,
}

impl Session {
    pub fn new(proxy: EventProxy, window_size: WindowSize) -> Self {
        let size = TermSize {
            columns: window_size.num_cols as usize,
            lines: window_size.num_lines as usize,
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        let term = Term::new(config, &size, proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        let mut options = tty::Options::default();
        options
            .env
            .insert("TERM".to_string(), "xterm-256color".to_string());
        let pty = tty::new(&options, window_size, 0).expect("PTY 생성 실패");

        let event_loop = PtyEventLoop::new(Arc::clone(&term), proxy, pty, false, false)
            .expect("PTY 이벤트 루프 생성 실패");
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();

        Self { term, notifier }
    }

    /// 키 입력 등을 PTY로 보낸다.
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.notifier.0.send(Msg::Input(Cow::Owned(bytes)));
    }

    /// 창 크기 변경을 그리드와 PTY 양쪽에 반영한다.
    pub fn resize(&self, window_size: WindowSize) {
        let _ = self.notifier.0.send(Msg::Resize(window_size));
        self.term.lock().resize(TermSize {
            columns: window_size.num_cols as usize,
            lines: window_size.num_lines as usize,
        });
    }
}
