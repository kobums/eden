//! Phase 0: headless로 PTY 위에 셸을 띄우고, 터미널 그리드 상태를 확인한다.
//!
//! 목표는 `셸 → PTY → VT 파서 → 그리드` 파이프라인이 alacritty_terminal로
//! 실제로 동작하는지 검증하는 것. 렌더러가 붙기 전까지의 코어 루프다.

use std::borrow::Cow;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;

/// 터미널 이벤트를 채널로 전달하는 리스너.
#[derive(Clone)]
struct EventProxy(mpsc::Sender<Event>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

/// 그리드 크기 (임시 고정값 — 렌더러가 생기면 창 크기에서 계산).
struct TermSize {
    columns: usize,
    lines: usize,
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

fn main() {
    let (event_tx, event_rx) = mpsc::channel();
    let proxy = EventProxy(event_tx);

    let size = TermSize { columns: 80, lines: 24 };
    let term = Term::new(Config::default(), &size, proxy.clone());
    let term = Arc::new(FairMutex::new(term));

    let window_size = WindowSize {
        num_cols: 80,
        num_lines: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let pty = tty::new(&tty::Options::default(), window_size, 0)
        .expect("PTY 생성 실패");

    let event_loop = EventLoop::new(Arc::clone(&term), proxy, pty, false, false)
        .expect("이벤트 루프 생성 실패");
    let notifier = Notifier(event_loop.channel());
    let _io_thread = event_loop.spawn();

    // 셸에 명령을 흘려보낸다.
    let input: Cow<'static, [u8]> = Cow::Owned(b"echo phase0-$((6*7))\r".to_vec());
    notifier.0.send(Msg::Input(input)).expect("PTY 입력 실패");

    // 셸이 출력할 시간을 준다 (임시 — 이후엔 Wakeup 이벤트 기반으로 전환).
    std::thread::sleep(Duration::from_millis(1500));

    // 그리드 내용을 덤프한다.
    let term = term.lock();
    let grid = term.grid();
    println!("--- grid dump ({}x{}) ---", grid.columns(), grid.screen_lines());
    let mut check = String::new();
    let mut line = String::new();
    let mut current_row = 0;
    for indexed in grid.display_iter() {
        if indexed.point.line.0 as usize != current_row {
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                println!("{trimmed}");
            }
            check.push('\n');
            line.clear();
            current_row = indexed.point.line.0 as usize;
        }
        line.push(indexed.c);
        check.push(indexed.c);
    }
    let trimmed = line.trim_end();
    if !trimmed.is_empty() {
        println!("{trimmed}");
    }
    println!("--- end ---");

    // 검증: 명령의 실행 결과가 그리드에 존재해야 한다.
    let found = check.contains("phase0-42");
    drop(term);

    // 남은 이벤트는 버린다 (Phase 1에서 Wakeup 기반 렌더 루프로 대체).
    while event_rx.try_recv().is_ok() {}

    let _ = notifier.0.send(Msg::Shutdown);

    if found {
        println!("OK: 셸 → PTY → 파서 → 그리드 파이프라인 동작 확인");
    } else {
        eprintln!("FAIL: 그리드에서 명령 출력을 찾지 못함");
        std::process::exit(1);
    }
}
