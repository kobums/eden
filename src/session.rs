//! 터미널 세션: PTY 위에 셸을 띄우고 자체 IO 루프로 그리드 상태를 유지한다.
//!
//! alacritty_terminal의 EventLoop 대신 자체 읽기/쓰기 스레드를 쓰는 이유:
//! PTY 바이트 스트림에서 OSC 133(셸 통합 마크)을 파서에 넣기 전에 가로채
//! 블록/프롬프트 경계를 기록하기 위해서다.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, OnResize, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::Processor;
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

/// OSC 133 마크 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    /// `133;A` — 프롬프트 시작
    PromptStart,
    /// `133;C` — 명령 출력 시작
    CommandStart,
    /// `133;D;<exit>` — 명령 종료
    CommandEnd(Option<i32>),
}

/// 셸 통합 마크. `abs_line`은 스크롤백을 포함한 절대 줄 번호로,
/// 줄이 히스토리로 밀려도 값이 유지된다 (히스토리 상한 초과 시 오차 발생 가능).
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    pub kind: MarkKind,
    pub abs_line: i64,
}

pub type Marks = Arc<Mutex<Vec<Mark>>>;

pub struct Session {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    pub marks: Marks,
    writer_tx: Sender<Vec<u8>>,
    pty: Arc<Mutex<tty::Pty>>,
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
        // zsh 셸 통합(OSC 133) 주입: ZDOTDIR을 우리 부트스트랩 디렉터리로 지정
        if let Some(shell_dir) = install_shell_integration() {
            if let Ok(orig) = std::env::var("ZDOTDIR") {
                options
                    .env
                    .insert("TERMDEV_ORIG_ZDOTDIR".to_string(), orig);
            }
            options.env.insert(
                "TERMDEV_INTEGRATION".to_string(),
                shell_dir.join("integration.zsh").display().to_string(),
            );
            options
                .env
                .insert("ZDOTDIR".to_string(), shell_dir.display().to_string());
        }
        let pty = tty::new(&options, window_size, 0).expect("PTY 생성 실패");

        // tty::new는 master fd를 논블로킹으로 만든다. 우리는 블로킹 스레드 IO를
        // 쓰므로 되돌린다 (dup된 fd끼리 파일 상태 플래그를 공유한다).
        let master_fd = pty.file().as_raw_fd();
        unsafe {
            let flags = libc::fcntl(master_fd, libc::F_GETFL, 0);
            libc::fcntl(master_fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
        }

        let read_file = pty.file().try_clone().expect("PTY fd 복제 실패");
        let write_file = pty.file().try_clone().expect("PTY fd 복제 실패");
        let pty = Arc::new(Mutex::new(pty));
        let marks: Marks = Arc::new(Mutex::new(Vec::new()));

        // 쓰기 스레드: 채널로 받은 바이트를 PTY에 쓴다.
        let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("pty-writer".into())
            .spawn(move || {
                let mut file = write_file;
                while let Ok(bytes) = writer_rx.recv() {
                    if file.write_all(&bytes).is_err() {
                        break;
                    }
                }
            })
            .expect("쓰기 스레드 생성 실패");

        // 읽기 스레드: PTY 출력을 OSC 133 스캐너를 거쳐 파서에 공급한다.
        {
            let term = Arc::clone(&term);
            let marks = Arc::clone(&marks);
            let proxy = proxy.clone();
            std::thread::Builder::new()
                .name("pty-reader".into())
                .spawn(move || {
                    reader_loop(read_file, term, marks, proxy);
                })
                .expect("읽기 스레드 생성 실패");
        }

        Self {
            term,
            marks,
            writer_tx,
            pty,
        }
    }

    /// 키 입력 등을 PTY로 보낸다.
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.writer_tx.send(bytes);
    }

    /// 창 크기 변경을 그리드와 PTY 양쪽에 반영한다.
    pub fn resize(&self, window_size: WindowSize) {
        self.pty.lock().unwrap().on_resize(window_size);
        self.term.lock().resize(TermSize {
            columns: window_size.num_cols as usize,
            lines: window_size.num_lines as usize,
        });
    }

    /// 이전(-1) / 다음(+1) 프롬프트로 화면을 점프한다 (OSC 133 A 마크 기반).
    pub fn jump_to_prompt(&self, direction: i32) {
        // 주의: marks 잠금을 들고 term을 잠그면 읽기 스레드와 교착한다.
        // 필요한 값만 복사하고 잠금을 푼 뒤 term을 잠근다.
        let prompts: Vec<i64> = {
            let marks = self.marks.lock().unwrap();
            marks
                .iter()
                .filter(|m| m.kind == MarkKind::PromptStart)
                .map(|m| m.abs_line)
                .collect()
        };

        let mut term = self.term.lock();
        let history = term.grid().history_size() as i64;
        let offset = term.grid().display_offset() as i64;
        let top_abs = history - offset;

        let target = if direction < 0 {
            prompts.iter().copied().filter(|abs| *abs < top_abs).max()
        } else {
            prompts.iter().copied().filter(|abs| *abs > top_abs).min()
        };

        match target {
            Some(abs) => {
                let new_offset = (history - abs).clamp(0, history);
                term.scroll_display(Scroll::Delta((new_offset - offset) as i32));
            }
            None if direction > 0 => term.scroll_display(Scroll::Bottom),
            None => {}
        }
    }
}

/// 셸 통합 스크립트를 캐시 디렉터리에 설치하고 그 경로를 돌려준다.
fn install_shell_integration() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".cache/terminal-dev/shell");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join(".zshenv"), include_str!("../shell/zshenv")).ok()?;
    std::fs::write(
        dir.join("integration.zsh"),
        include_str!("../shell/integration.zsh"),
    )
    .ok()?;
    Some(dir)
}

// --- PTY 읽기 루프 + OSC 133 스캐너 ---

const OSC133_PREFIX: &[u8] = b"\x1b]133;";

fn reader_loop(
    mut file: File,
    term: Arc<FairMutex<Term<EventProxy>>>,
    marks: Marks,
    proxy: EventProxy,
) {
    let mut parser = Processor::new();
    let mut scanner = Osc133Scanner::default();
    let mut buf = [0u8; 65536];

    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                {
                    let mut term = term.lock();
                    scanner.process(&mut term, &mut parser, &marks, &buf[..n]);
                }
                proxy.send_event(Event::Wakeup);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // 셸 종료 시 EIO
            Err(_) => break,
        }
    }
    proxy.send_event(Event::Exit);
}

#[derive(Default)]
enum ScanState {
    #[default]
    Normal,
    /// OSC 133 본문 수집 중 (`ESC ] 133 ;` 이후, BEL 또는 ST까지)
    Collect {
        payload: Vec<u8>,
        esc_seen: bool,
    },
}

#[derive(Default)]
struct Osc133Scanner {
    state: ScanState,
    /// 청크 경계에 걸린 OSC 133 프리픽스 후보
    carry: Vec<u8>,
}

impl Osc133Scanner {
    fn process(
        &mut self,
        term: &mut Term<EventProxy>,
        parser: &mut Processor,
        marks: &Marks,
        chunk: &[u8],
    ) {
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(chunk);
        let mut pos = 0;

        loop {
            match &mut self.state {
                ScanState::Normal => {
                    let rest = &data[pos..];
                    if rest.is_empty() {
                        break;
                    }
                    if let Some(idx) = find_subsequence(rest, OSC133_PREFIX) {
                        // 프리픽스 이전까지는 그대로 파서에 공급
                        parser.advance(term, &rest[..idx]);
                        pos += idx + OSC133_PREFIX.len();
                        self.state = ScanState::Collect {
                            payload: Vec::new(),
                            esc_seen: false,
                        };
                    } else {
                        // 청크 끝이 프리픽스의 일부라면 다음 청크로 이월
                        let keep = longest_prefix_suffix(rest, OSC133_PREFIX);
                        parser.advance(term, &rest[..rest.len() - keep]);
                        self.carry = rest[rest.len() - keep..].to_vec();
                        break;
                    }
                }
                ScanState::Collect { payload, esc_seen } => {
                    let mut done = false;
                    while pos < data.len() {
                        let b = data[pos];
                        pos += 1;
                        if *esc_seen {
                            // ST(ESC \) 종료. ESC 뒤 다른 바이트는 비정상 — 그냥 종료 처리.
                            done = true;
                            break;
                        }
                        match b {
                            0x07 => {
                                done = true;
                                break;
                            }
                            0x1b => *esc_seen = true,
                            _ => payload.push(b),
                        }
                    }
                    if done {
                        let payload = std::mem::take(payload);
                        self.state = ScanState::Normal;
                        record_mark(term, marks, &payload);
                    } else {
                        // 청크 끝 — 다음 청크에서 이어서 수집
                        break;
                    }
                }
            }
        }
    }
}

/// OSC 133 페이로드(`A`, `C`, `D;0` 등)를 마크로 기록한다.
fn record_mark(term: &Term<EventProxy>, marks: &Marks, payload: &[u8]) {
    let kind = match payload.first() {
        Some(b'A') => MarkKind::PromptStart,
        Some(b'C') => MarkKind::CommandStart,
        Some(b'D') => {
            let exit = payload
                .get(2..)
                .and_then(|s| std::str::from_utf8(s).ok())
                .and_then(|s| s.parse().ok());
            MarkKind::CommandEnd(exit)
        }
        // B(프롬프트 끝) 등은 아직 사용하지 않음
        _ => return,
    };

    let grid = term.grid();
    let abs_line = grid.history_size() as i64 + grid.cursor.point.line.0 as i64;
    let mut marks = marks.lock().unwrap();
    // 같은 줄에 같은 종류가 중복 기록되는 것(프롬프트 다시 그리기 등)은 무시
    if let Some(last) = marks.last() {
        if last.kind == kind && last.abs_line == abs_line {
            return;
        }
    }
    marks.push(Mark { kind, abs_line });

    // TERMDEV_DEBUG_MARKS=1 로 실행하면 마크 기록을 stderr로 확인할 수 있다.
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *DEBUG.get_or_init(|| std::env::var_os("TERMDEV_DEBUG_MARKS").is_some()) {
        eprintln!("[mark] {kind:?} abs_line={abs_line}");
    }
}

/// `haystack`에서 `needle`의 첫 위치.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `data`의 접미사 중 `pattern`의 진접두사인 최장 길이.
fn longest_prefix_suffix(data: &[u8], pattern: &[u8]) -> usize {
    let max = (pattern.len() - 1).min(data.len());
    for len in (1..=max).rev() {
        if data[data.len() - len..] == pattern[..len] {
            return len;
        }
    }
    0
}
