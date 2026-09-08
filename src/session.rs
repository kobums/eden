//! 터미널 세션: PTY 위에 셸을 띄우고 자체 IO 루프로 그리드 상태를 유지한다.
//!
//! alacritty_terminal의 EventLoop 대신 자체 읽기/쓰기 스레드를 쓰는 이유:
//! PTY 바이트 스트림에서 OSC 133(셸 통합 마크)을 파서에 넣기 전에 가로채
//! 블록/프롬프트 경계를 기록하기 위해서다.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use winit::event_loop::EventLoopProxy;

use crate::mux::{self, MuxClient, MuxMsg};
use crate::osc::{
    self, OSC7_PREFIX, OSC133_PREFIX, collect_osc_payloads, find_subsequence,
    longest_prefix_suffix, parse_mark_kind,
};

pub use crate::osc::MarkKind;

/// 앱 이벤트: 터미널 이벤트(페인 ID 태깅), AI 생성 결과, 또는 Quake 전역 핫키.
pub enum AppEvent {
    Term(usize, Event),
    /// OSC 133 D — 명령 하나가 끝났다 (reader 스레드에서 옴).
    ///
    /// 알림을 낼지는 App(메인 스레드)이 판단한다 — 포커스 상태가 App에 있고,
    /// reader 스레드에서 AppKit을 부르면 안 되기 때문에 사실만 전달한다.
    CommandFinished {
        pane_id: usize,
        /// 직전 C 마크부터의 소요 시간
        duration: Duration,
        /// 종료 코드 (`D;<exit>`)
        exit: Option<i32>,
    },
    /// OSC 9(iTerm2)·OSC 777(rxvt) 알림 시퀀스. title이 None이면 OSC 9.
    Notify {
        pane_id: usize,
        title: Option<String>,
        body: String,
    },
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

    /// 명령 완료 사실을 App으로 보낸다. 알림 여부 판단은 App이 한다.
    fn send_command_finished(&self, duration: Duration, exit: Option<i32>) {
        let _ = self.proxy.send_event(AppEvent::CommandFinished {
            pane_id: self.tab_id,
            duration,
            exit,
        });
    }

    /// OSC 9/777 알림을 App으로 보낸다. 포커스 조건 판단도 App이 한다.
    fn send_notify(&self, title: Option<String>, body: String) {
        let _ = self.proxy.send_event(AppEvent::Notify {
            pane_id: self.tab_id,
            title,
            body,
        });
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

/// 셸 통합 마크. `abs_line`은 스크롤백을 포함한 절대 줄 번호로,
/// 줄이 히스토리로 밀려도 값이 유지된다 (히스토리 상한 초과 시 오차 발생 가능).
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    pub kind: MarkKind,
    pub abs_line: i64,
    /// 기록 시각. 소요 시간 계산에만 쓰므로 벽시계가 아니라 단조 시계면
    /// 충분하다 (직렬화하지 않는다).
    pub at: Instant,
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

/// Term 설정. 순수 함수로 분리해 소켓·스레드 없이 Term만으로 프로토콜
/// 왕복 테스트를 할 수 있다.
///
/// `kitty_keyboard`를 켜면 crate 파서가 kitty keyboard protocol의 상태
/// 머신(query `CSI ? u`·push `CSI > flags u`·pop `CSI < u`·set, 모드 스택,
/// 대체 스크린 분리)을 전부 처리하고 현재 플래그를 `term.mode()`로 노출한다.
/// 끄면 해당 시퀀스를 전부 무시하므로 advertise 자체가 일어나지 않는다 —
/// "켰는데 인코딩이 틀린" 반쪽 상태를 만들지 않기 위한 eden 설정 스위치는
/// 이 값 하나로 충분하다.
fn term_config(scrollback: usize, kitty_keyboard: bool) -> Config {
    Config {
        scrolling_history: scrollback,
        kitty_keyboard,
        ..Config::default()
    }
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
    pub fn new(
        proxy: EventProxy,
        window_size: WindowSize,
        scrollback: usize,
        kitty_keyboard: bool,
    ) -> Self {
        let _ = mux::ensure_daemon();
        let (client, read_stream) = MuxClient::create(window_size).expect("mux 세션 생성 실패");
        Self::build(
            proxy,
            window_size,
            scrollback,
            kitty_keyboard,
            client,
            read_stream,
            // 새 세션은 리플레이가 없다 — 알림 감시를 건너뛸 이유도 없다.
            false,
        )
    }

    /// 데몬의 기존 세션에 다시 붙는다 (detach 후 복원).
    pub fn attach(
        proxy: EventProxy,
        id: u64,
        window_size: WindowSize,
        scrollback: usize,
        kitty_keyboard: bool,
    ) -> Option<Self> {
        let (client, read_stream) = MuxClient::attach(id, window_size).ok()?;
        Some(Self::build(
            proxy,
            window_size,
            scrollback,
            kitty_keyboard,
            client,
            read_stream,
            true,
        ))
    }

    /// 현재 살아있는 세션 ID 목록 + 데몬 boot id (구버전 데몬이면 None).
    pub fn list() -> (Vec<u64>, Option<u64>) {
        MuxClient::list().unwrap_or_default()
    }

    /// 데몬 세션의 ID. 레이아웃 저장(layout.json)의 leaf 값으로 쓴다.
    pub fn id(&self) -> u64 {
        self.client.id()
    }

    fn build(
        proxy: EventProxy,
        window_size: WindowSize,
        scrollback: usize,
        kitty_keyboard: bool,
        client: MuxClient,
        read_stream: std::os::unix::net::UnixStream,
        attached: bool,
    ) -> Self {
        let size = TermSize {
            columns: window_size.num_cols as usize,
            lines: window_size.num_lines as usize,
        };
        let term = Term::new(
            term_config(scrollback, kitty_keyboard),
            &size,
            proxy.clone(),
        );
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
                    reader_loop(read_stream, term, marks, cwd, proxy, attached);
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
                    if let Some(block) = blocks.last_mut()
                        && block.cmd_abs.is_none()
                    {
                        block.cmd_abs = Some(mark.abs_line);
                    }
                }
                MarkKind::CommandEnd(exit) => {
                    if let Some(block) = blocks.last_mut()
                        && block.end_abs.is_none()
                    {
                        block.end_abs = Some(mark.abs_line);
                        block.exit = exit;
                    }
                }
            }
        }
        blocks
    }

    /// 명령이 실행 중인가 — 마지막 마크가 C(CommandStart)다.
    pub fn is_running(&self) -> bool {
        let marks = self.marks.lock().unwrap();
        matches!(marks.last().map(|m| m.kind), Some(MarkKind::CommandStart))
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

/// Synchronized output (DEC private mode 2026)의 진입/종료 시퀀스.
/// 공통 프리픽스 뒤 'h'(진입) 또는 'l'(종료).
const SYNC_PREFIX: &[u8] = b"\x1b[?2026";

fn reader_loop(
    mut stream: std::os::unix::net::UnixStream,
    term: Arc<FairMutex<Term<EventProxy>>>,
    marks: Marks,
    cwd: Arc<Mutex<String>>,
    proxy: EventProxy,
    attached: bool,
) {
    let mut parser = Processor::new();
    let mut scanner = Osc133Scanner::default();
    // synchronized output 상태: sync 중에는 redraw를 억제하고, 종료 시 한 번에 그린다.
    let mut sync = false;
    let mut sync_carry: Vec<u8> = Vec::new();
    let mut cwd_carry: Vec<u8> = Vec::new();
    // OSC 9/777 감시. 프리픽스가 둘이라 carry도 둘이다 — 한 버퍼를 공유하면
    // 한쪽 스캐너가 잘라낸 조각을 다른 쪽이 놓친다.
    let mut notify9_carry: Vec<u8> = Vec::new();
    let mut notify777_carry: Vec<u8> = Vec::new();
    // attach 리플레이(과거 출력)의 알림 시퀀스를 다시 울리지 않기 위한 플래그.
    // 데몬은 리플레이 버퍼를 첫 Output 메시지 하나로 보낸다 (mux.rs의
    // `Chunk::Data(s.replay.clone())`)는 것에 의존한다. cwd(마지막 값이
    // 유효)와 D 마크(리플레이는 소요 시간 ≈ 0이라 임계값에 걸러짐)는
    // 건너뛸 필요가 없다.
    let mut replay_pending = attached;

    loop {
        match mux::read_msg(&mut stream) {
            Ok(MuxMsg::Output(data)) => {
                {
                    let mut term = term.lock();
                    scanner.process(&mut term, &mut parser, &marks, &proxy, &data);
                }
                // OSC 7 작업 디렉터리 감시 (상태바용)
                if let Some(path) = watch_cwd(&mut cwd_carry, &data) {
                    *cwd.lock().unwrap() = path;
                }
                // OSC 9/777 알림 감시
                if !std::mem::take(&mut replay_pending) {
                    for (title, body) in
                        watch_notify(&mut notify9_carry, &mut notify777_carry, &data)
                    {
                        proxy.send_notify(title, body);
                    }
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
/// 돌려준다. 여러 개면 마지막 값만 유효하다. 청크 경계는 carry가 처리한다.
fn watch_cwd(carry: &mut Vec<u8>, chunk: &[u8]) -> Option<String> {
    collect_osc_payloads(carry, chunk, OSC7_PREFIX)
        .iter()
        .filter_map(|p| osc::parse_osc7(p))
        .next_back()
}

/// iTerm2 알림: `ESC ] 9 ; <본문> BEL|ST`
const OSC9_PREFIX: &[u8] = b"\x1b]9;";
/// rxvt 알림: `ESC ] 777 ; notify ; <제목> ; <본문> BEL|ST`
const OSC777_PREFIX: &[u8] = b"\x1b]777;";

/// 청크에서 OSC 9/777 알림을 전부 수집한다 — (제목, 본문). 제목 None이면 OSC 9.
///
/// cwd(마지막 값만 유효)와 달리 알림은 하나하나가 별개라 Vec으로 돌려준다.
/// 같은 청크에서 9와 777이 섞이면 종류별로 묶여 나온다 (순서 뒤섞임 허용 —
/// 실전에서 한 청크에 두 종류가 같이 오는 일은 없다시피 하다).
fn watch_notify(
    carry9: &mut Vec<u8>,
    carry777: &mut Vec<u8>,
    chunk: &[u8],
) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    for payload in collect_osc_payloads(carry9, chunk, OSC9_PREFIX) {
        if let Some(body) = parse_osc9(&payload) {
            out.push((None, body));
        }
    }
    for payload in collect_osc_payloads(carry777, chunk, OSC777_PREFIX) {
        if let Some((title, body)) = parse_osc777(&payload) {
            out.push((Some(title), body));
        }
    }
    out
}

/// OSC 9 본문. 사람에게 보여줄 텍스트이므로 UTF-8이 아니거나 비어 있으면 버린다.
fn parse_osc9(payload: &[u8]) -> Option<String> {
    let body = std::str::from_utf8(payload).ok()?.trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// OSC 777 `notify;<제목>;<본문>` → (제목, 본문).
/// `notify` 이외의 서브커맨드는 무시한다. 두 번째 `;`까지만 구분자라서
/// 본문 속 `;`는 그대로 남는다. 본문이 생략되면 빈 문자열.
fn parse_osc777(payload: &[u8]) -> Option<(String, String)> {
    let s = std::str::from_utf8(payload).ok()?;
    let rest = s.strip_prefix("notify;")?;
    let (title, body) = rest.split_once(';').unwrap_or((rest, ""));
    Some((title.to_string(), body.to_string()))
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
        proxy: &EventProxy,
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
                        record_mark(term, marks, proxy, &payload);
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
fn record_mark(term: &Term<EventProxy>, marks: &Marks, proxy: &EventProxy, payload: &[u8]) {
    let Some(kind) = parse_mark_kind(payload) else {
        return;
    };

    let grid = term.grid();
    let abs_line = grid.history_size() as i64 + grid.cursor.point.line.0 as i64;
    let mut marks = marks.lock().unwrap();
    // 같은 줄에 같은 종류가 중복 기록되는 것(프롬프트 다시 그리기 등)은 무시
    if let Some(last) = marks.last()
        && last.kind == kind
        && last.abs_line == abs_line
    {
        return;
    }
    marks.push(Mark {
        kind,
        abs_line,
        at: Instant::now(),
    });

    // D 마크면 소요 시간을 계산해 App으로 보낸다. 여기서(reader 스레드)
    // 알림을 직접 띄우지 않는 이유: 포커스 상태는 App에 있고, AppKit은
    // 메인 스레드 밖에서 부르면 안 된다. 중복 마크는 위에서 걸러졌으므로
    // 프롬프트 다시 그리기로 이벤트가 두 번 가는 일은 없다.
    if let MarkKind::CommandEnd(exit) = kind
        && let Some(duration) = last_command_duration(&marks)
    {
        proxy.send_command_finished(duration, exit);
    }

    // EDEN_DEBUG_MARKS=1 로 실행하면 마크 기록을 stderr로 확인할 수 있다.
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *DEBUG.get_or_init(|| std::env::var_os("EDEN_DEBUG_MARKS").is_some()) {
        eprintln!("[mark] {kind:?} abs_line={abs_line}");
    }
}

/// 마지막 마크가 D(CommandEnd)일 때, 짝이 되는 직전 C(CommandStart)부터의
/// 소요 시간. C 없이 D만 왔거나(수동 `printf '\e]133;D\a'` 등) 되짚다
/// 다른 D를 먼저 만나면 짝이 없는 것이므로 None.
fn last_command_duration(marks: &[Mark]) -> Option<Duration> {
    let (last, rest) = marks.split_last()?;
    if !matches!(last.kind, MarkKind::CommandEnd(_)) {
        return None;
    }
    for mark in rest.iter().rev() {
        match mark.kind {
            // Instant는 미래가 앞서면 duration_since가 0으로 포화한다 —
            // 단조 시계라 실제로는 일어나지 않지만 패닉 걱정이 없다.
            MarkKind::CommandStart => return Some(last.at.duration_since(mark.at)),
            MarkKind::CommandEnd(_) => return None,
            MarkKind::PromptStart => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::TermMode;

    /// 파서가 보내는 응답(PtyWrite 등)을 붙잡는 테스트용 리스너.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<Event>>>);

    impl EventListener for Capture {
        fn send_event(&self, event: Event) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn kitty_term(enabled: bool) -> (Term<Capture>, Processor, Capture) {
        let capture = Capture::default();
        let size = TermSize {
            columns: 80,
            lines: 24,
        };
        let term = Term::new(term_config(100, enabled), &size, capture.clone());
        (term, Processor::new(), capture)
    }

    #[test]
    fn kitty_push_sets_mode_flags_and_pop_clears_them() {
        let (mut term, mut parser, _capture) = kitty_term(true);
        assert!(
            !term.mode().intersects(TermMode::KITTY_KEYBOARD_PROTOCOL),
            "초기 상태는 플래그 없음"
        );

        // CSI > 1 u — DISAMBIGUATE push
        parser.advance(&mut term, b"\x1b[>1u");
        assert!(term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
        assert!(!term.mode().contains(TermMode::REPORT_EVENT_TYPES));

        // CSI < u — pop
        parser.advance(&mut term, b"\x1b[<u");
        assert!(
            !term.mode().intersects(TermMode::KITTY_KEYBOARD_PROTOCOL),
            "pop 후에는 플래그가 사라져야 한다"
        );
    }

    #[test]
    fn kitty_push_all_five_flags() {
        let (mut term, mut parser, _capture) = kitty_term(true);
        parser.advance(&mut term, b"\x1b[>31u");
        let mode = *term.mode();
        assert!(mode.contains(TermMode::DISAMBIGUATE_ESC_CODES));
        assert!(mode.contains(TermMode::REPORT_EVENT_TYPES));
        assert!(mode.contains(TermMode::REPORT_ALTERNATE_KEYS));
        assert!(mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC));
        assert!(mode.contains(TermMode::REPORT_ASSOCIATED_TEXT));
    }

    #[test]
    fn kitty_disabled_ignores_the_protocol_entirely() {
        // eden 설정 kitty-keyboard = off의 실체: crate가 시퀀스를 무시해
        // advertise도 모드 변경도 일어나지 않는다.
        let (mut term, mut parser, _capture) = kitty_term(false);
        parser.advance(&mut term, b"\x1b[>31u");
        assert!(!term.mode().intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
    }

    #[test]
    fn kitty_mode_is_separate_per_screen() {
        // 대체 스크린 진입/이탈 시 모드가 갈린다 — on_key가 매 키마다
        // mode()를 읽어야 하는 이유의 회귀 테스트.
        let (mut term, mut parser, _capture) = kitty_term(true);
        parser.advance(&mut term, b"\x1b[>1u");
        assert!(term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));

        parser.advance(&mut term, b"\x1b[?1049h"); // 대체 스크린 진입
        assert!(
            !term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES),
            "대체 스크린은 자체 모드 스택으로 시작한다"
        );

        parser.advance(&mut term, b"\x1b[?1049l"); // 이탈
        assert!(
            term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES),
            "주 스크린으로 돌아오면 원래 모드가 복원된다"
        );
    }

    #[test]
    fn kitty_query_reports_current_flags() {
        let (mut term, mut parser, capture) = kitty_term(true);
        parser.advance(&mut term, b"\x1b[?u");
        parser.advance(&mut term, b"\x1b[>5u\x1b[?u");
        let replies: Vec<String> = capture
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::PtyWrite(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            replies,
            vec!["\x1b[?0u".to_string(), "\x1b[?5u".to_string()],
            "query 응답이 PtyWrite 이벤트로 나와야 앱이 지원을 감지한다"
        );
    }

    fn mark_at(kind: MarkKind, t0: Instant, secs: u64) -> Mark {
        Mark {
            kind,
            abs_line: 0,
            at: t0 + Duration::from_secs(secs),
        }
    }

    #[test]
    fn duration_needs_a_matching_command_start() {
        let t0 = Instant::now();
        // C 없이 D만 온 경우 (수동 printf 등) — 시작점을 모른다.
        let marks = [
            mark_at(MarkKind::PromptStart, t0, 0),
            mark_at(MarkKind::CommandEnd(Some(0)), t0, 5),
        ];
        assert_eq!(last_command_duration(&marks), None);

        // 마지막 마크가 D가 아니면 계산할 것이 없다.
        let marks = [
            mark_at(MarkKind::PromptStart, t0, 0),
            mark_at(MarkKind::CommandStart, t0, 1),
        ];
        assert_eq!(last_command_duration(&marks), None);
        assert_eq!(last_command_duration(&[]), None);
    }

    #[test]
    fn duration_is_time_from_c_to_d() {
        let t0 = Instant::now();
        let marks = [
            mark_at(MarkKind::PromptStart, t0, 0),
            mark_at(MarkKind::CommandStart, t0, 1),
            mark_at(MarkKind::CommandEnd(Some(0)), t0, 6),
        ];
        assert_eq!(last_command_duration(&marks), Some(Duration::from_secs(5)));
    }

    #[test]
    fn duration_of_consecutive_commands_uses_the_latest_pair() {
        let t0 = Instant::now();
        let marks = [
            mark_at(MarkKind::PromptStart, t0, 0),
            mark_at(MarkKind::CommandStart, t0, 1),
            mark_at(MarkKind::CommandEnd(Some(0)), t0, 100),
            mark_at(MarkKind::PromptStart, t0, 100),
            mark_at(MarkKind::CommandStart, t0, 110),
            mark_at(MarkKind::CommandEnd(Some(1)), t0, 112),
        ];
        assert_eq!(last_command_duration(&marks), Some(Duration::from_secs(2)));

        // D가 연달아 오면 두 번째 D는 짝이 없다 (직전 C는 첫 D가 소비했다).
        let marks = [
            mark_at(MarkKind::CommandStart, t0, 1),
            mark_at(MarkKind::CommandEnd(Some(0)), t0, 5),
            mark_at(MarkKind::CommandEnd(Some(0)), t0, 6),
        ];
        assert_eq!(last_command_duration(&marks), None);
    }

    // --- OSC 9/777 알림 파싱 (Phase 17) ---

    /// carry 없이 한 청크를 감시하는 테스트 헬퍼.
    fn notify_once(chunk: &[u8]) -> Vec<(Option<String>, String)> {
        watch_notify(&mut Vec::new(), &mut Vec::new(), chunk)
    }

    #[test]
    fn osc9_terminated_by_bel_or_st() {
        assert_eq!(
            notify_once(b"\x1b]9;done!\x07"),
            vec![(None, "done!".to_string())]
        );
        assert_eq!(
            notify_once(b"\x1b]9;done!\x1b\\"),
            vec![(None, "done!".to_string())],
            "ST(ESC \\) 종료"
        );
        assert_eq!(notify_once(b"\x1b]9;\x07"), vec![], "빈 본문은 무시");
    }

    #[test]
    fn osc777_notify_with_title_and_body() {
        assert_eq!(
            notify_once(b"\x1b]777;notify;Build;finished\x07"),
            vec![(Some("Build".to_string()), "finished".to_string())]
        );
        // 본문 속 ';'는 구분자가 아니다.
        assert_eq!(
            notify_once(b"\x1b]777;notify;T;a;b\x1b\\"),
            vec![(Some("T".to_string()), "a;b".to_string())]
        );
        // 본문 생략은 허용, notify 이외의 서브커맨드는 무시.
        assert_eq!(
            notify_once(b"\x1b]777;notify;only-title\x07"),
            vec![(Some("only-title".to_string()), String::new())]
        );
        assert_eq!(notify_once(b"\x1b]777;other;x;y\x07"), vec![]);
    }

    #[test]
    fn notify_sequences_split_across_chunks_are_carried() {
        let mut c9 = Vec::new();
        let mut c777 = Vec::new();
        // 프리픽스 한가운데에서 끊김
        assert_eq!(watch_notify(&mut c9, &mut c777, b"ab\x1b]"), vec![]);
        // 본문 한가운데에서 또 끊김
        assert_eq!(watch_notify(&mut c9, &mut c777, b"9;he"), vec![]);
        assert_eq!(
            watch_notify(&mut c9, &mut c777, b"llo\x07cd"),
            vec![(None, "hello".to_string())]
        );

        // ST의 ESC까지만 온 채 끊기는 경우 — ESC \ 완성 대기
        let mut c9 = Vec::new();
        let mut c777 = Vec::new();
        assert_eq!(watch_notify(&mut c9, &mut c777, b"\x1b]9;hi\x1b"), vec![]);
        assert_eq!(
            watch_notify(&mut c9, &mut c777, b"\\"),
            vec![(None, "hi".to_string())]
        );
    }

    #[test]
    fn notify_ignores_invalid_utf8() {
        assert_eq!(notify_once(b"\x1b]9;\xff\xfe\x07"), vec![]);
        assert_eq!(notify_once(b"\x1b]777;notify;\xff;x\x07"), vec![]);
        // 한글은 당연히 통과해야 한다.
        assert_eq!(
            notify_once("\x1b]9;빌드 완료\x07".as_bytes()),
            vec![(None, "빌드 완료".to_string())]
        );
    }

    #[test]
    fn multiple_notifications_in_one_chunk() {
        assert_eq!(
            notify_once(b"\x1b]9;one\x07mid\x1b]9;two\x07"),
            vec![(None, "one".to_string()), (None, "two".to_string())]
        );
    }
}
