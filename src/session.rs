//! 터미널 세션: PTY 위에 셸을 띄우고 자체 IO 루프로 그리드 상태를 유지한다.
//!
//! alacritty_terminal의 EventLoop 대신 자체 읽기/쓰기 스레드를 쓰는 이유:
//! PTY 바이트 스트림에서 OSC 133(셸 통합 마크)을 파서에 넣기 전에 가로채
//! 블록/프롬프트 경계를 기록하기 위해서다.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use winit::event_loop::EventLoopProxy;

use crate::mux::{self, MuxClient, MuxMsg};

/// 앱 이벤트: 터미널 이벤트(페인 ID 태깅), AI 생성 결과, 또는 Quake 전역 핫키.
pub enum AppEvent {
    Term(usize, Event),
    AiResult {
        pane_id: usize,
        seq: u64,
        result: Result<String, String>,
    },
    /// Quake 드롭다운 토글 (전역 핫키)
    QuakeToggle,
    /// 상태바 갱신용 주기적 틱 (~1초)
    Tick,
}

/// 터미널 이벤트를 (페인 ID와 함께) winit 이벤트 루프로 전달하는 리스너.
#[derive(Clone)]
pub struct EventProxy {
    proxy: EventLoopProxy<AppEvent>,
    tab_id: usize,
}

impl EventProxy {
    pub fn new(proxy: EventLoopProxy<AppEvent>, tab_id: usize) -> Self {
        Self { proxy, tab_id }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.proxy.send_event(AppEvent::Term(self.tab_id, event));
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

/// OSC 133 마크로부터 도출된 명령 블록 (프롬프트 + 명령 + 출력 단위).
#[derive(Clone, Copy, Debug)]
pub struct Block {
    /// 프롬프트 시작 줄 (A 마크)
    pub start_abs: i64,
    /// 명령 출력 시작 줄 (C 마크) — None이면 명령이 실행되지 않은 프롬프트
    pub cmd_abs: Option<i64>,
    /// 명령 종료 줄 (D 마크, 다음 프롬프트가 그려질 줄) — None이면 실행 중
    pub end_abs: Option<i64>,
    /// 종료 코드 (D;<exit>)
    pub exit: Option<i32>,
}

pub struct Session {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    pub marks: Marks,
    /// OSC 7으로 보고된 현재 작업 디렉터리 (상태바용).
    pub cwd: Arc<Mutex<String>>,
    client: Arc<MuxClient>,
}

impl Session {
    /// mux 데몬에 새 세션을 만들어 붙는다.
    pub fn new(proxy: EventProxy, window_size: WindowSize, scrollback: usize) -> Self {
        let _ = mux::ensure_daemon();
        let (client, read_stream) = MuxClient::create(window_size).expect("mux 세션 생성 실패");
        Self::build(proxy, window_size, scrollback, client, read_stream)
    }

    /// 데몬의 기존 세션에 다시 붙는다 (detach 후 복원).
    pub fn attach(
        proxy: EventProxy,
        id: u64,
        window_size: WindowSize,
        scrollback: usize,
    ) -> Option<Self> {
        let (client, read_stream) = MuxClient::attach(id, window_size).ok()?;
        Some(Self::build(
            proxy,
            window_size,
            scrollback,
            client,
            read_stream,
        ))
    }

    /// 현재 살아있는 세션 ID 목록.
    pub fn list() -> Vec<u64> {
        MuxClient::list().unwrap_or_default()
    }

    /// 데몬 세션의 ID.
    #[allow(dead_code)] // 향후 레이아웃 저장/복원에 사용
    pub fn id(&self) -> u64 {
        self.client.id()
    }

    fn build(
        proxy: EventProxy,
        window_size: WindowSize,
        scrollback: usize,
        client: MuxClient,
        read_stream: std::os::unix::net::UnixStream,
    ) -> Self {
        let size = TermSize {
            columns: window_size.num_cols as usize,
            lines: window_size.num_lines as usize,
        };
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let term = Term::new(config, &size, proxy.clone());
        let term = Arc::new(FairMutex::new(term));
        let marks: Marks = Arc::new(Mutex::new(Vec::new()));
        let cwd: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

        // 읽기 스레드: 데몬이 보내는 출력(리플레이 + 라이브)을
        // OSC 133 스캐너를 거쳐 파서에 공급한다.
        {
            let term = Arc::clone(&term);
            let marks = Arc::clone(&marks);
            let cwd = Arc::clone(&cwd);
            let proxy = proxy.clone();
            std::thread::Builder::new()
                .name("mux-reader".into())
                .spawn(move || {
                    reader_loop(read_stream, term, marks, cwd, proxy);
                })
                .expect("읽기 스레드 생성 실패");
        }

        Self {
            term,
            marks,
            cwd,
            client: Arc::new(client),
        }
    }

    /// 현재 작업 디렉터리 (OSC 7). 없으면 빈 문자열.
    pub fn cwd(&self) -> String {
        self.cwd.lock().unwrap().clone()
    }

    /// 키 입력 등을 데몬 세션(PTY)으로 보낸다.
    pub fn write(&self, bytes: Vec<u8>) {
        self.client.write(&bytes);
    }

    /// 세션을 완전히 종료한다 (셸 kill).
    pub fn kill(&self) {
        self.client.kill();
    }

    /// 창 크기 변경을 그리드와 데몬 PTY 양쪽에 반영한다.
    pub fn resize(&self, window_size: WindowSize) {
        self.client.resize(window_size);
        self.term.lock().resize(TermSize {
            columns: window_size.num_cols as usize,
            lines: window_size.num_lines as usize,
        });
    }

    /// 마크 목록에서 블록들을 도출한다.
    pub fn blocks(&self) -> Vec<Block> {
        let marks = self.marks.lock().unwrap();
        let mut blocks: Vec<Block> = Vec::new();
        for mark in marks.iter() {
            match mark.kind {
                MarkKind::PromptStart => blocks.push(Block {
                    start_abs: mark.abs_line,
                    cmd_abs: None,
                    end_abs: None,
                    exit: None,
                }),
                MarkKind::CommandStart => {
                    if let Some(block) = blocks.last_mut() {
                        if block.cmd_abs.is_none() {
                            block.cmd_abs = Some(mark.abs_line);
                        }
                    }
                }
                MarkKind::CommandEnd(exit) => {
                    if let Some(block) = blocks.last_mut() {
                        if block.end_abs.is_none() {
                            block.end_abs = Some(mark.abs_line);
                            block.exit = exit;
                        }
                    }
                }
            }
        }
        blocks
    }

    /// 마지막으로 완료된 명령의 출력 범위 [시작, 끝] (절대 줄 번호).
    pub fn last_output_range(&self) -> Option<(i64, i64)> {
        self.blocks().iter().rev().find_map(|block| {
            let cmd = block.cmd_abs?;
            let end = block.end_abs?;
            (end > cmd).then_some((cmd, end - 1))
        })
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

    /// 절대 줄 `abs`가 화면에 없으면 가운데로 오도록 스크롤한다.
    ///
    /// 이미 화면 안이면 아무것도 하지 않는다 — 증분 검색에서 타이핑마다
    /// 화면이 떨리는 것을 막는다. `jump_to_prompt`가 대상을 화면 맨 위에
    /// 두는 것과 달리 가운데에 두는 이유는 검색 히트에는 주변 맥락이
    /// 필요하기 때문이다.
    ///
    /// 뷰포트 위치 계산은 `jump_to_prompt`와 같다: `scroll_display`가 상대
    /// 델타만 받으므로 원하는 절대 offset에서 현재 offset을 뺀다.
    pub fn scroll_to_abs(&self, abs: i64) {
        let mut term = self.term.lock();
        let history = term.grid().history_size() as i64;
        let offset = term.grid().display_offset() as i64;
        let screen = Dimensions::screen_lines(term.grid()) as i64;

        let top_abs = history - offset;
        if abs >= top_abs && abs < top_abs + screen {
            return; // 이미 보인다
        }

        let target = (history - abs + screen / 2).clamp(0, history);
        term.scroll_display(Scroll::Delta((target - offset) as i32));
    }
}

// --- mux 읽기 루프 + OSC 133 스캐너 ---

const OSC133_PREFIX: &[u8] = b"\x1b]133;";

/// Synchronized output (DEC private mode 2026)의 진입/종료 시퀀스.
/// 공통 프리픽스 뒤 'h'(진입) 또는 'l'(종료).
const SYNC_PREFIX: &[u8] = b"\x1b[?2026";

fn reader_loop(
    mut stream: std::os::unix::net::UnixStream,
    term: Arc<FairMutex<Term<EventProxy>>>,
    marks: Marks,
    cwd: Arc<Mutex<String>>,
    proxy: EventProxy,
) {
    let mut parser = Processor::new();
    let mut scanner = Osc133Scanner::default();
    // synchronized output 상태: sync 중에는 redraw를 억제하고, 종료 시 한 번에 그린다.
    let mut sync = false;
    let mut sync_carry: Vec<u8> = Vec::new();
    let mut cwd_carry: Vec<u8> = Vec::new();

    loop {
        match mux::read_msg(&mut stream) {
            Ok(MuxMsg::Output(data)) => {
                {
                    let mut term = term.lock();
                    scanner.process(&mut term, &mut parser, &marks, &data);
                }
                // OSC 7 작업 디렉터리 감시 (상태바용)
                if let Some(path) = watch_cwd(&mut cwd_carry, &data) {
                    *cwd.lock().unwrap() = path;
                }
                // mode 2026 진입/종료를 감시해 프레임 원자성을 확보한다.
                sync = watch_sync(sync, &mut sync_carry, &data);
                if !sync {
                    proxy.send_event(Event::Wakeup);
                }
            }
            // 셸 종료
            Ok(MuxMsg::Exit) => break,
            // 소켓 종료(데몬 죽음 등)
            Err(_) => break,
        }
    }
    proxy.send_event(Event::Exit);
}

/// 바이트 스트림에서 OSC 7(`ESC ] 7 ; file://host/path BEL|ST`)을 찾아 경로를
/// 돌려준다. 청크 경계에 걸친 시퀀스를 위해 carry를 유지한다.
fn watch_cwd(carry: &mut Vec<u8>, chunk: &[u8]) -> Option<String> {
    const PREFIX: &[u8] = b"\x1b]7;";
    carry.extend_from_slice(chunk);
    let mut result = None;

    loop {
        let Some(start) = find_subsequence(carry, PREFIX) else {
            // 프리픽스 없음: 경계에 걸린 프리픽스 후보만 남긴다
            let keep = longest_prefix_suffix(carry, PREFIX);
            let cut = carry.len() - keep;
            carry.drain(..cut);
            break;
        };
        let body = start + PREFIX.len();
        // 종료: BEL(0x07) 또는 ST(ESC \)
        let Some(rel) = carry[body..].iter().position(|&b| b == 0x07 || b == 0x1b) else {
            // 종료 미도착: 프리픽스부터 보관하고 대기
            carry.drain(..start);
            break;
        };
        let term_pos = body + rel;
        let end = if carry[term_pos] == 0x1b {
            if term_pos + 1 >= carry.len() {
                carry.drain(..start); // ST 미완성, 대기
                break;
            }
            term_pos + 2
        } else {
            term_pos + 1
        };
        if let Some(path) = parse_osc7(&carry[body..term_pos]) {
            result = Some(path);
        }
        carry.drain(..end);
    }

    // carry 무한 증가 방지
    if carry.len() > 8192 {
        let cut = carry.len() - 8192;
        carry.drain(..cut);
    }
    result
}

/// `file://hostname/path` → `/path` (퍼센트 디코딩).
fn parse_osc7(payload: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(payload).ok()?;
    let rest = s.strip_prefix("file://")?;
    // 호스트 이후 첫 '/'부터가 경로
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => rest,
    };
    // 퍼센트 디코딩 (%20 등)
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = |c: u8| (c as char).to_digit(16);
            if let (Some(a), Some(b)) = (h(bytes[i + 1]), h(bytes[i + 2])) {
                out.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// 청크에서 mode 2026 진입(`h`)/종료(`l`)를 감지해 최신 sync 상태를 돌려준다.
/// 청크 경계에 걸친 시퀀스를 위해 carry에 최대 7바이트를 이월한다.
fn watch_sync(prev: bool, carry: &mut Vec<u8>, chunk: &[u8]) -> bool {
    let mut scan = std::mem::take(carry);
    scan.extend_from_slice(chunk);

    let mut state = prev;
    let plen = SYNC_PREFIX.len();
    if scan.len() > plen {
        for i in 0..=scan.len() - plen - 1 {
            if &scan[i..i + plen] == SYNC_PREFIX {
                match scan[i + plen] {
                    b'h' => state = true,
                    b'l' => state = false,
                    _ => {}
                }
            }
        }
    }

    // 다음 경계용 carry: 끝 (plen)바이트 유지
    let keep = plen.min(scan.len());
    *carry = scan[scan.len() - keep..].to_vec();
    state
}

#[derive(Default)]
enum ScanState {
    #[default]
    Normal,
    /// OSC 133 본문 수집 중 (`ESC ] 133 ;` 이후, BEL 또는 ST까지)
    Collect { payload: Vec<u8>, esc_seen: bool },
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
