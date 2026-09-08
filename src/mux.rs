//! 세션 지속성을 위한 mux(멀티플렉서) 데몬 + 클라이언트.
//!
//! 셸과 PTY는 GUI가 아니라 별도 데몬 프로세스가 소유한다. 데몬은 각 세션의
//! 출력 바이트를 리플레이 버퍼에 축적하고, 붙어 있는 클라이언트(GUI)에게
//! 실시간으로 전달한다. GUI를 닫아도(detach) 데몬은 셸을 살려두고, 다시 열면
//! (attach) 리플레이 버퍼를 재생해 화면·스크롤백·실행 중인 프로그램을 복원한다.
//!
//! Term은 여전히 GUI(클라이언트)에 있으므로 선택·스크롤·블록·AI 등 기존 기능은
//! 바이트 소스만 소켓으로 바뀔 뿐 그대로 동작한다.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty;

use crate::osc::ShellState;

// --- 프레임 태그 ---
// 클라이언트 → 데몬
const CREATE: u8 = 0x01; // cols u16, lines u16, px u16, py u16
const ATTACH: u8 = 0x02; // id u64
const INPUT: u8 = 0x03; // raw bytes
const RESIZE: u8 = 0x04; // cols u16, lines u16, px u16, py u16
const LIST: u8 = 0x05; // (empty)
const KILL: u8 = 0x06; // (empty) — 현재 세션 종료
// 클라이언트 → 데몬: 구독 없이 한 번 묻고 끊는 CLI용 프레임 (Phase 22).
// 연결 하나에 요청 하나 — 응답을 받으면 양쪽 다 닫는다.
const SEND: u8 = 0x07; // id u64, raw bytes — 지정 세션의 PTY에 입력
const PEEK: u8 = 0x08; // id u64 — 리플레이 버퍼 스냅샷 (리사이즈·구독 없음)
const LIST_INFO: u8 = 0x09; // (empty) — 세션 상세 목록
const SPAWN: u8 = 0x0A; // cols u16, lines u16, px u16, py u16, cwd bytes — 붙지 않고 세션 생성
const KILL_ID: u8 = 0x0B; // id u64 — 지정 세션 종료
// 데몬 → 클라이언트
const ATTACHED: u8 = 0x81; // id u64
const OUTPUT: u8 = 0x82; // raw bytes
const SESSION_LIST: u8 = 0x83; // count u32, ids u64..., boot u64 (꼬리 필드 — 구버전 데몬 응답에는 없다)
const EXIT: u8 = 0x85; // (empty) — 셸 종료
const OK: u8 = 0x86; // u8 — 1 성공 / 0 실패 (세션 없음 등)
const REPLAY: u8 = 0x87; // cols u16, lines u16, raw bytes
const SESSION_INFO: u8 = 0x88; // count u32, SessionInfo… (encode_info 참고)
const CREATED: u8 = 0x89; // id u64

/// mux 제어 소켓 경로.
pub fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    socket_path_for(&home)
}

/// `sockaddr_un.sun_path`에 담을 수 있는 최대 경로 길이 (macOS 104바이트,
/// 종단 NUL 포함이므로 실제 경로는 103자까지).
const SUN_PATH_MAX: usize = 103;

/// HOME 기준 소켓 경로. 기본은 `~/.cache/eden/mux/control.sock`인데,
/// 유닉스 소켓 경로에는 `sun_path` 길이 제한이 있어 HOME이 길면 bind/connect가
/// 실패한다 — 격리된 HOME으로 띄우는 테스트에서 `Session::new`가 패닉으로
/// 드러난 실버그다 (plan.md). 그 경우 `/tmp` 아래의 짧은 경로로 폴백한다.
/// 데몬과 클라이언트가 같은 규칙으로 계산하므로 항상 서로를 찾고, HOME 해시가
/// 붙어 있어 서로 다른 HOME(=다른 데몬)의 격리도 유지된다.
fn socket_path_for(home: &str) -> PathBuf {
    let path = PathBuf::from(home).join(".cache/eden/mux/control.sock");
    if path.as_os_str().len() <= SUN_PATH_MAX {
        return path;
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!(
        "/tmp/eden-mux-{uid}-{:016x}.sock",
        fnv1a(home.as_bytes())
    ))
}

/// FNV-1a 64비트 해시. 암호학적 강도가 필요 없는 경로 구분용.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// --- 프레임 IO ---

fn write_frame(stream: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    let len = (payload.len() + 1) as u32;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&[tag])?;
    stream.write_all(payload)?;
    stream.flush()
}

fn read_frame(stream: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "빈 프레임"));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    let tag = buf[0];
    Ok((tag, buf[1..].to_vec()))
}

fn window_size(cols: u16, lines: u16, px: u16, py: u16) -> WindowSize {
    WindowSize {
        num_cols: cols,
        num_lines: lines,
        cell_width: px,
        cell_height: py,
    }
}

fn encode_size(ws: &WindowSize) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[0..2].copy_from_slice(&ws.num_cols.to_le_bytes());
    b[2..4].copy_from_slice(&ws.num_lines.to_le_bytes());
    b[4..6].copy_from_slice(&ws.cell_width.to_le_bytes());
    b[6..8].copy_from_slice(&ws.cell_height.to_le_bytes());
    b
}

fn decode_size(p: &[u8]) -> Option<WindowSize> {
    if p.len() < 8 {
        return None;
    }
    Some(window_size(
        u16::from_le_bytes([p[0], p[1]]),
        u16::from_le_bytes([p[2], p[3]]),
        u16::from_le_bytes([p[4], p[5]]),
        u16::from_le_bytes([p[6], p[7]]),
    ))
}

// ======================= 클라이언트 =======================

/// 데몬 세션에 연결된 클라이언트 핸들. write/resize/kill용 쓰기 스트림을 보유한다.
pub struct MuxClient {
    id: u64,
    writer: Mutex<UnixStream>,
}

impl MuxClient {
    /// 새 세션을 만들고 붙는다.
    pub fn create(ws: WindowSize) -> io::Result<(Self, UnixStream)> {
        let mut stream = connect()?;
        write_frame(&mut stream, CREATE, &encode_size(&ws))?;
        Self::finish_attach(stream)
    }

    /// 기존 세션에 다시 붙는다.
    pub fn attach(id: u64, ws: WindowSize) -> io::Result<(Self, UnixStream)> {
        let mut stream = connect()?;
        let mut payload = id.to_le_bytes().to_vec();
        payload.extend_from_slice(&encode_size(&ws));
        write_frame(&mut stream, ATTACH, &payload)?;
        Self::finish_attach(stream)
    }

    fn finish_attach(mut stream: UnixStream) -> io::Result<(Self, UnixStream)> {
        let (tag, payload) = read_frame(&mut stream)?;
        if tag != ATTACHED || payload.len() < 8 {
            return Err(io::Error::other("attach 실패"));
        }
        let id = u64::from_le_bytes(payload[..8].try_into().unwrap());
        let writer = stream.try_clone()?;
        Ok((
            Self {
                id,
                writer: Mutex::new(writer),
            },
            stream, // 읽기용 (Output/Exit 프레임)
        ))
    }

    /// 살아있는 세션 ID 목록과 데몬 boot id를 조회한다.
    ///
    /// boot id는 layout.json이 "이 세션 ID들이 어느 데몬 세대의 것인지"를
    /// 대조하는 데 쓴다 — 세션 ID는 데몬 재시작 시 1부터 재발급되기 때문.
    /// 구버전 데몬은 boot id를 보내지 않는다 → None (대조를 포기한다).
    /// 이 하위호환이 없으면 데몬 업그레이드 중(구버전 데몬 + 신버전 GUI)에 깨진다.
    pub fn list() -> io::Result<(Vec<u64>, Option<u64>)> {
        let mut stream = connect()?;
        write_frame(&mut stream, LIST, &[])?;
        let (tag, payload) = read_frame(&mut stream)?;
        if tag != SESSION_LIST || payload.len() < 4 {
            return Ok((Vec::new(), None));
        }
        let count = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize;
        let mut ids = Vec::with_capacity(count);
        for i in 0..count {
            let off = 4 + i * 8;
            if off + 8 <= payload.len() {
                ids.push(u64::from_le_bytes(
                    payload[off..off + 8].try_into().unwrap(),
                ));
            }
        }
        let tail = 4 + count * 8;
        let boot = (payload.len() >= tail + 8)
            .then(|| u64::from_le_bytes(payload[tail..tail + 8].try_into().unwrap()));
        Ok((ids, boot))
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = write_frame(&mut *w, INPUT, bytes);
        }
    }

    pub fn resize(&self, ws: WindowSize) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = write_frame(&mut *w, RESIZE, &encode_size(&ws));
        }
    }

    /// 세션을 완전히 종료한다 (셸 kill).
    pub fn kill(&self) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = write_frame(&mut *w, KILL, &[]);
        }
    }
}

/// 세션 하나의 상세 — `eden list`가 보여주는 것.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: u64,
    /// 셸 프로세스 PID
    pub pid: u32,
    pub cols: u16,
    pub lines: u16,
    /// 명령 실행 중 (OSC 133 C 이후 D 전)
    pub running: bool,
    /// 마지막 명령의 종료 코드
    pub last_exit: Option<i32>,
    pub cwd: String,
    /// 실행 중(또는 마지막) 명령줄. 셸 통합이 안 보내면 빈 문자열.
    pub command: String,
    /// 시작된 명령 수 / 끝난 명령 수 (`ShellState` 참고).
    pub commands: u64,
    pub completions: u64,
}

/// `SESSION_INFO` 페이로드 인코딩: `[u32 count]` 뒤에 세션마다
/// `[u32 len][레코드 본문]`. 본문은 `id u64 · pid u32 · cols u16 · lines u16 ·
/// running u8 · exit_present u8 · exit i32 · cwd (u16 len + bytes) ·
/// command (u16 len + bytes) · commands u64 · completions u64`.
///
/// 레코드에 길이가 붙어 있으므로 새 필드는 본문 꼬리에 붙이기만 하면 된다 —
/// `decode_info`는 아는 필드까지 읽고 나머지는 길이만큼 건너뛰므로 구버전
/// 클라이언트가 신버전 데몬의 응답을 읽을 수 있다 (반대 방향은 없는 필드를
/// 기본값으로 채우면 되지만, 지금은 필요 없어 잘린 레코드는 버린다).
fn encode_info(infos: &[SessionInfo]) -> Vec<u8> {
    let mut out = (infos.len() as u32).to_le_bytes().to_vec();
    for info in infos {
        let mut body = Vec::new();
        body.extend_from_slice(&info.id.to_le_bytes());
        body.extend_from_slice(&info.pid.to_le_bytes());
        body.extend_from_slice(&info.cols.to_le_bytes());
        body.extend_from_slice(&info.lines.to_le_bytes());
        body.push(info.running as u8);
        body.push(info.last_exit.is_some() as u8);
        body.extend_from_slice(&info.last_exit.unwrap_or(0).to_le_bytes());
        for field in [&info.cwd, &info.command] {
            let bytes = &field.as_bytes()[..field.len().min(u16::MAX as usize)];
            body.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
            body.extend_from_slice(bytes);
        }
        body.extend_from_slice(&info.commands.to_le_bytes());
        body.extend_from_slice(&info.completions.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
    }
    out
}

/// `encode_info`의 역. 잘린 페이로드는 읽을 수 있는 레코드까지만 돌려준다.
fn decode_info(payload: &[u8]) -> Vec<SessionInfo> {
    struct Cursor<'a>(&'a [u8]);
    impl<'a> Cursor<'a> {
        fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
            let bytes: [u8; N] = self.0.get(..N)?.try_into().ok()?;
            self.0 = &self.0[N..];
            Some(bytes)
        }
        fn bytes(&mut self, len: usize) -> Option<&'a [u8]> {
            let bytes = self.0.get(..len)?;
            self.0 = &self.0[len..];
            Some(bytes)
        }
        fn string(&mut self) -> Option<String> {
            let len = u16::from_le_bytes(self.take()?) as usize;
            Some(String::from_utf8_lossy(self.bytes(len)?).into_owned())
        }
    }
    fn record(body: &[u8]) -> Option<SessionInfo> {
        let mut cur = Cursor(body);
        let id = u64::from_le_bytes(cur.take()?);
        let pid = u32::from_le_bytes(cur.take()?);
        let cols = u16::from_le_bytes(cur.take()?);
        let lines = u16::from_le_bytes(cur.take()?);
        let running = cur.take::<1>()?[0] != 0;
        let exit_present = cur.take::<1>()?[0] != 0;
        let exit = i32::from_le_bytes(cur.take()?);
        let cwd = cur.string()?;
        let command = cur.string()?;
        let commands = u64::from_le_bytes(cur.take()?);
        let completions = u64::from_le_bytes(cur.take()?);
        // 이 뒤에 남는 바이트는 우리가 모르는 신버전 필드 — 무시한다.
        Some(SessionInfo {
            id,
            pid,
            cols,
            lines,
            running,
            last_exit: exit_present.then_some(exit),
            cwd,
            command,
            commands,
            completions,
        })
    }
    let mut cur = Cursor(payload);
    let Some(count) = cur.take().map(u32::from_le_bytes) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for _ in 0..count {
        let Some(info) = cur
            .take()
            .map(u32::from_le_bytes)
            .and_then(|len| cur.bytes(len as usize))
            .and_then(record)
        else {
            break;
        };
        out.push(info);
    }
    out
}

/// 구독 없이 한 번 묻고 끊는 CLI용 요청. 연결 하나에 요청 하나.
///
/// 구버전 데몬은 모르는 태그를 받으면 응답 없이 연결을 닫으므로, 읽기가
/// EOF로 끝나면 "데몬이 구버전"이라는 뜻이다 — `read_reply`가 그 경우를
/// 사람이 읽을 수 있는 오류로 바꾼다.
fn request(tag: u8, payload: &[u8]) -> io::Result<(u8, Vec<u8>)> {
    let mut stream = connect()?;
    write_frame(&mut stream, tag, payload)?;
    match read_frame(&mut stream) {
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Err(io::Error::other(
            "mux 데몬이 이 명령을 모릅니다 (구버전 데몬). 세션을 모두 닫아 데몬을 재시작하세요",
        )),
        other => other,
    }
}

/// `OK` 응답을 bool로.
fn ok_reply(reply: (u8, Vec<u8>)) -> io::Result<bool> {
    match reply {
        (OK, payload) => Ok(payload.first().copied().unwrap_or(0) == 1),
        (tag, _) => Err(io::Error::other(format!("예상 밖 응답 0x{tag:02x}"))),
    }
}

/// 세션 상세 목록 (id 오름차순).
pub fn list_info() -> io::Result<Vec<SessionInfo>> {
    match request(LIST_INFO, &[])? {
        (SESSION_INFO, payload) => Ok(decode_info(&payload)),
        (tag, _) => Err(io::Error::other(format!("예상 밖 응답 0x{tag:02x}"))),
    }
}

/// 지정 세션의 PTY에 바이트를 쓴다. 세션이 없으면 Ok(false).
pub fn send(id: u64, bytes: &[u8]) -> io::Result<bool> {
    let mut payload = id.to_le_bytes().to_vec();
    payload.extend_from_slice(bytes);
    ok_reply(request(SEND, &payload)?)
}

/// 리플레이 버퍼 스냅샷 — (cols, lines, bytes). 세션이 없으면 Ok(None).
pub fn peek(id: u64) -> io::Result<Option<(u16, u16, Vec<u8>)>> {
    match request(PEEK, &id.to_le_bytes())? {
        (REPLAY, payload) if payload.len() >= 4 => {
            let cols = u16::from_le_bytes([payload[0], payload[1]]);
            let lines = u16::from_le_bytes([payload[2], payload[3]]);
            Ok(Some((cols, lines, payload[4..].to_vec())))
        }
        (OK, _) => Ok(None),
        (tag, _) => Err(io::Error::other(format!("예상 밖 응답 0x{tag:02x}"))),
    }
}

/// 붙지 않고 세션만 만든다. GUI는 다음 포커스 때 이 세션을 탭으로 붙인다.
pub fn spawn(ws: WindowSize, cwd: Option<&str>) -> io::Result<u64> {
    let mut payload = encode_size(&ws).to_vec();
    payload.extend_from_slice(cwd.unwrap_or("").as_bytes());
    match request(SPAWN, &payload)? {
        (CREATED, payload) if payload.len() >= 8 => {
            Ok(u64::from_le_bytes(payload[..8].try_into().unwrap()))
        }
        (OK, _) => Err(io::Error::other(
            "세션 생성 실패 (작업 디렉터리를 확인하세요)",
        )),
        (tag, _) => Err(io::Error::other(format!("예상 밖 응답 0x{tag:02x}"))),
    }
}

/// 지정 세션의 셸에 SIGHUP. 세션이 없으면 Ok(false).
pub fn kill_id(id: u64) -> io::Result<bool> {
    ok_reply(request(KILL_ID, &id.to_le_bytes())?)
}

/// 데몬 읽기 스트림에서 Output/Exit 프레임을 꺼낸다.
pub enum MuxMsg {
    Output(Vec<u8>),
    Exit,
}

pub fn read_msg(stream: &mut UnixStream) -> io::Result<MuxMsg> {
    loop {
        let (tag, payload) = read_frame(stream)?;
        match tag {
            OUTPUT => return Ok(MuxMsg::Output(payload)),
            EXIT => return Ok(MuxMsg::Exit),
            _ => continue, // 알 수 없는 프레임은 무시
        }
    }
}

fn connect() -> io::Result<UnixStream> {
    UnixStream::connect(socket_path())
}

/// 데몬이 없으면 띄우고, 소켓이 응답할 때까지 기다린다.
pub fn ensure_daemon() -> io::Result<()> {
    if connect().is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    // 표준 입출력을 끊는다. 물려받으면 `id=$(eden new)` 같은 셸 캡처가 데몬이
    // 파이프 쓰기 끝을 쥐고 있어 영영 EOF를 못 받는다. 데몬은 어차피 아무것도
    // 출력하지 않는다.
    std::process::Command::new(exe)
        .arg("--daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    // 소켓이 준비될 때까지 폴링 (최대 ~3초)
    for _ in 0..150 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        if connect().is_ok() {
            return Ok(());
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "데몬 시작 실패"))
}

// ======================= 데몬 =======================

/// 구독자에게 보내는 메시지.
#[derive(Clone)]
enum Chunk {
    Data(Vec<u8>),
    Ended,
}

/// 최대 리플레이 버퍼 크기 (초과 시 앞부분을 개행 경계에서 잘라낸다).
const REPLAY_CAP: usize = 2 * 1024 * 1024;

struct DaemonSession {
    pty: tty::Pty,
    replay: Vec<u8>,
    subscribers: Vec<Sender<Chunk>>,
    /// 마지막으로 반영한 PTY 크기 — `eden capture`가 같은 크기의 Term으로
    /// 리플레이를 재생해야 화면이 맞는다.
    size: WindowSize,
    /// 출력에서 도출한 셸 상태 (`eden list`·`eden wait`용).
    shell: ShellState,
}

impl DaemonSession {
    fn info(&self, id: u64) -> SessionInfo {
        SessionInfo {
            id,
            pid: self.pty.child().id(),
            cols: self.size.num_cols,
            lines: self.size.num_lines,
            running: self.shell.running,
            last_exit: self.shell.last_exit,
            cwd: self.shell.cwd.clone(),
            command: self.shell.command.clone(),
            commands: self.shell.commands,
            completions: self.shell.completions,
        }
    }

    fn hangup(&self) {
        // 셸에 SIGHUP. 리더 스레드가 EOF를 보고 구독자에게 Ended를 알리고
        // 레지스트리에서 제거한다.
        let pid = self.pty.child().id();
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGHUP);
        }
    }
}

type Registry = Arc<Mutex<HashMap<u64, Arc<Mutex<DaemonSession>>>>>;

/// 데몬 진입점. 소켓에 바인드하고 연결을 받는다. 반환하지 않는다.
pub fn run_daemon() -> ! {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // 이미 살아있는 데몬이 있으면 종료
    if connect().is_ok() {
        std::process::exit(0);
    }
    // 죽은 소켓 파일 정리
    let _ = std::fs::remove_file(&path);

    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(_) => std::process::exit(0),
    };

    // 세션(부모 프로세스)로부터 독립
    unsafe {
        libc::setsid();
    }

    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
    let next_id = Arc::new(AtomicU64::new(1));
    // 데몬 시작 시각 = boot id. 세션 ID는 재시작마다 1부터 재발급되므로,
    // 죽은 데몬 시절의 layout.json이 새 데몬의 엉뚱한 세션과 매칭되는 것을
    // 클라이언트가 이 값으로 걸러낸다. 단조성은 필요 없고 세대 구분만 하면
    // 되므로 벽시계(epoch ms)로 충분하다.
    let boot_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let registry = Arc::clone(&registry);
        let next_id = Arc::clone(&next_id);
        std::thread::spawn(move || handle_connection(stream, registry, next_id, boot_id));
    }
    std::process::exit(0);
}

fn handle_connection(
    mut stream: UnixStream,
    registry: Registry,
    next_id: Arc<AtomicU64>,
    boot_id: u64,
) {
    let Ok((tag, payload)) = read_frame(&mut stream) else {
        return;
    };

    match tag {
        LIST => {
            let ids: Vec<u64> = registry.lock().unwrap().keys().copied().collect();
            let mut out = (ids.len() as u32).to_le_bytes().to_vec();
            for id in ids {
                out.extend_from_slice(&id.to_le_bytes());
            }
            // boot id는 꼬리에만 붙인다 — 구버전 클라이언트는 count만큼 읽고
            // 나머지를 무시하므로 프레임 형식이 하위호환이다.
            out.extend_from_slice(&boot_id.to_le_bytes());
            let _ = write_frame(&mut stream, SESSION_LIST, &out);
        }
        CREATE => {
            let Some(ws) = decode_size(&payload) else {
                return;
            };
            let id = next_id.fetch_add(1, Ordering::SeqCst);
            let Ok(session) = spawn_session(ws, id, Arc::clone(&registry), None) else {
                return;
            };
            registry.lock().unwrap().insert(id, Arc::clone(&session));
            serve_subscriber(stream, id, session);
        }
        ATTACH => {
            if payload.len() < 8 {
                return;
            }
            let id = u64::from_le_bytes(payload[..8].try_into().unwrap());
            let session = registry.lock().unwrap().get(&id).map(Arc::clone);
            if let Some(session) = session {
                // 붙을 때 크기 반영
                if let Some(ws) = decode_size(&payload[8..]) {
                    let mut s = session.lock().unwrap();
                    s.pty.on_resize(ws);
                    s.size = ws;
                }
                serve_subscriber(stream, id, session);
            }
        }
        // --- 아래는 CLI용 단발 요청. 응답 하나를 쓰고 연결을 끝낸다. ---
        LIST_INFO => {
            let sessions: Vec<(u64, Arc<Mutex<DaemonSession>>)> = registry
                .lock()
                .unwrap()
                .iter()
                .map(|(id, s)| (*id, Arc::clone(s)))
                .collect();
            let mut infos: Vec<SessionInfo> = sessions
                .iter()
                .map(|(id, s)| s.lock().unwrap().info(*id))
                .collect();
            infos.sort_by_key(|i| i.id);
            let _ = write_frame(&mut stream, SESSION_INFO, &encode_info(&infos));
        }
        SEND => {
            let Some(id) = session_id(&payload) else {
                return;
            };
            let session = registry.lock().unwrap().get(&id).map(Arc::clone);
            let ok = session.is_some_and(|session| {
                let s = session.lock().unwrap();
                s.pty
                    .file()
                    .try_clone()
                    .and_then(|mut f| f.write_all(&payload[8..]))
                    .is_ok()
            });
            let _ = write_frame(&mut stream, OK, &[ok as u8]);
        }
        PEEK => {
            let Some(id) = session_id(&payload) else {
                return;
            };
            let session = registry.lock().unwrap().get(&id).map(Arc::clone);
            match session {
                Some(session) => {
                    let s = session.lock().unwrap();
                    let mut out = s.size.num_cols.to_le_bytes().to_vec();
                    out.extend_from_slice(&s.size.num_lines.to_le_bytes());
                    out.extend_from_slice(&s.replay);
                    let _ = write_frame(&mut stream, REPLAY, &out);
                }
                None => {
                    let _ = write_frame(&mut stream, OK, &[0]);
                }
            }
        }
        SPAWN => {
            let Some(ws) = decode_size(&payload) else {
                return;
            };
            let cwd = std::str::from_utf8(&payload[8..])
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            let id = next_id.fetch_add(1, Ordering::SeqCst);
            match spawn_session(ws, id, Arc::clone(&registry), cwd) {
                Ok(session) => {
                    registry.lock().unwrap().insert(id, session);
                    let _ = write_frame(&mut stream, CREATED, &id.to_le_bytes());
                }
                Err(_) => {
                    let _ = write_frame(&mut stream, OK, &[0]);
                }
            }
        }
        KILL_ID => {
            let Some(id) = session_id(&payload) else {
                return;
            };
            let session = registry.lock().unwrap().get(&id).map(Arc::clone);
            let ok = session.is_some_and(|session| {
                session.lock().unwrap().hangup();
                true
            });
            let _ = write_frame(&mut stream, OK, &[ok as u8]);
        }
        _ => {}
    }
}

/// 페이로드 앞 8바이트의 세션 ID.
fn session_id(payload: &[u8]) -> Option<u64> {
    payload
        .get(..8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

/// PTY에 셸을 띄우고, 출력을 리플레이 버퍼 + 구독자에게 뿌리는 리더 스레드를 시작한다.
///
/// `cwd`가 None이면 홈 디렉터리. 데몬은 Dock에서 실행된 앱이 띄워 cwd가 `/`라,
/// 지정하지 않으면 셸이 그걸 상속해 `/`에서 시작하기 때문에 명시한다.
/// 디렉터리가 없으면 PTY 생성이 실패하고 Err — 데몬을 죽이지 않는다.
fn spawn_session(
    ws: WindowSize,
    id: u64,
    registry: Registry,
    cwd: Option<PathBuf>,
) -> io::Result<Arc<Mutex<DaemonSession>>> {
    let mut options = tty::Options {
        working_directory: cwd.or_else(|| std::env::var("HOME").ok().map(PathBuf::from)),
        ..Default::default()
    };
    options
        .env
        .insert("TERM".to_string(), "xterm-256color".to_string());
    // 셸 통합(OSC 133) 주입 — 클라이언트가 스캔한다
    if let Some(shell_dir) = install_shell_integration() {
        if let Ok(orig) = std::env::var("ZDOTDIR") {
            options.env.insert("EDEN_ORIG_ZDOTDIR".to_string(), orig);
        }
        options.env.insert(
            "EDEN_INTEGRATION".to_string(),
            shell_dir.join("integration.zsh").display().to_string(),
        );
        options
            .env
            .insert("ZDOTDIR".to_string(), shell_dir.display().to_string());
    }
    let pty = tty::new(&options, ws, 0)?;

    // master fd를 블로킹으로 (tty::new가 논블로킹으로 만든다)
    let master_fd = pty.file().as_raw_fd();
    unsafe {
        let flags = libc::fcntl(master_fd, libc::F_GETFL, 0);
        libc::fcntl(master_fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
    }
    let mut read_file = pty.file().try_clone().expect("fd 복제 실패");

    let session = Arc::new(Mutex::new(DaemonSession {
        pty,
        replay: Vec::new(),
        subscribers: Vec::new(),
        size: ws,
        shell: ShellState::default(),
    }));

    let reader_session = Arc::clone(&session);
    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            match read_file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = &buf[..n];
                    let mut s = reader_session.lock().unwrap();
                    s.replay.extend_from_slice(data);
                    trim_replay(&mut s.replay);
                    s.shell.feed(data);
                    let chunk = Chunk::Data(data.to_vec());
                    s.subscribers.retain(|tx| tx.send(chunk.clone()).is_ok());
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        // 셸 종료 → 구독자에게 알리고 레지스트리에서 제거
        {
            let mut s = reader_session.lock().unwrap();
            for tx in &s.subscribers {
                let _ = tx.send(Chunk::Ended);
            }
            s.subscribers.clear();
        }
        registry.lock().unwrap().remove(&id);
        if registry.lock().unwrap().is_empty() {
            std::process::exit(0);
        }
    });

    Ok(session)
}

/// 한 클라이언트 연결을 세션 구독자로 서비스한다.
/// 출력 스레드(세션→소켓) + 입력 루프(소켓→PTY)로 구성.
fn serve_subscriber(stream: UnixStream, id: u64, session: Arc<Mutex<DaemonSession>>) {
    let (tx, rx) = mpsc::channel::<Chunk>();

    // 구독 등록: 락을 잡은 채 리플레이 전송 + 구독자 추가 → 라이브 바이트 유실 방지
    {
        let mut s = session.lock().unwrap();
        if !s.replay.is_empty() {
            let _ = tx.send(Chunk::Data(s.replay.clone()));
        }
        s.subscribers.push(tx);
    }

    // ATTACHED 응답
    let mut out_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    if write_frame(&mut out_stream, ATTACHED, &id.to_le_bytes()).is_err() {
        return;
    }

    // 출력 스레드
    std::thread::spawn(move || {
        for chunk in rx {
            let ok = match chunk {
                Chunk::Data(data) => write_frame(&mut out_stream, OUTPUT, &data).is_ok(),
                Chunk::Ended => {
                    let _ = write_frame(&mut out_stream, EXIT, &[]);
                    false
                }
            };
            if !ok {
                break; // 소켓 닫힘(detach) 또는 종료 → rx 드롭 → 리더가 구독 해제
            }
        }
    });

    // 입력 루프 (이 스레드)
    let mut in_stream = stream;
    let mut write_file = {
        let s = session.lock().unwrap();
        s.pty.file().try_clone().ok()
    };
    loop {
        match read_frame(&mut in_stream) {
            Ok((INPUT, data)) => {
                if let Some(f) = write_file.as_mut()
                    && f.write_all(&data).is_err()
                {
                    break;
                }
            }
            Ok((RESIZE, payload)) => {
                if let Some(ws) = decode_size(&payload) {
                    let mut s = session.lock().unwrap();
                    s.pty.on_resize(ws);
                    s.size = ws;
                }
            }
            Ok((KILL, _)) => {
                session.lock().unwrap().hangup();
                break;
            }
            Ok(_) => {}
            Err(_) => break, // detach
        }
    }
    // 연결 종료 = detach. 세션은 살아있다 (KILL이었으면 셸이 종료되며 정리된다).
}

/// 리플레이 버퍼가 상한을 넘으면 앞부분을 개행 경계에서 잘라낸다.
fn trim_replay(replay: &mut Vec<u8>) {
    if replay.len() <= REPLAY_CAP {
        return;
    }
    let overflow = replay.len() - REPLAY_CAP;
    // overflow 이후 첫 개행까지 버린다 (이스케이프 시퀀스 중간 절단 완화)
    let cut = replay[overflow..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|p| overflow + p + 1)
        .unwrap_or(overflow);
    replay.drain(..cut);
}

/// 셸 통합 스크립트를 캐시 디렉터리에 설치한다 (session.rs와 동일 경로).
///
/// zsh만 자동 주입된다(ZDOTDIR 우회). bash용 스크립트도 함께 깔지만 부르지는
/// 않는다 — bash에는 안전한 주입 지점이 없기 때문이다. 로그인 셸은 `--rcfile`을
/// 무시하고, `--rcfile`을 쓰려고 로그인 셸을 포기하면 `/etc/profile`
/// (path_helper)을 건너뛰어 PATH가 조용히 달라진다. 사용자가 직접 한 줄
/// 추가하는 쪽이 정직하다 (docs/features.md 참고).
fn install_shell_integration() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".cache/eden/shell");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join(".zshenv"), include_str!("../shell/zshenv")).ok()?;
    std::fs::write(
        dir.join("integration.zsh"),
        include_str!("../shell/integration.zsh"),
    )
    .ok()?;
    // bash는 자동 주입하지 않지만, 사용자가 source할 수 있게 같이 깔아둔다.
    std::fs::write(
        dir.join("integration.bash"),
        include_str!("../shell/integration.bash"),
    )
    .ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<SessionInfo> {
        vec![
            SessionInfo {
                id: 1,
                pid: 4242,
                cols: 120,
                lines: 40,
                running: true,
                last_exit: None,
                cwd: "/Users/me/프로젝트".into(),
                command: "cargo test -- --nocapture".into(),
                commands: 12,
                completions: 11,
            },
            SessionInfo {
                id: 7,
                pid: 1,
                cols: 80,
                lines: 24,
                running: false,
                last_exit: Some(130),
                cwd: String::new(),
                command: String::new(),
                commands: 0,
                completions: 0,
            },
        ]
    }

    #[test]
    fn session_info_round_trips() {
        let infos = sample();
        assert_eq!(decode_info(&encode_info(&infos)), infos);
        assert_eq!(decode_info(&encode_info(&[])), Vec::<SessionInfo>::new());
    }

    #[test]
    fn truncated_info_yields_the_complete_records_only() {
        let bytes = encode_info(&sample());
        // 두 번째 레코드 중간에서 잘리면 첫 레코드만 나온다
        let cut = bytes.len() - 3;
        assert_eq!(decode_info(&bytes[..cut]), sample()[..1].to_vec());
        assert!(decode_info(&[]).is_empty());
        assert!(decode_info(&[9, 0, 0, 0]).is_empty());
    }

    #[test]
    fn unknown_trailing_record_fields_are_skipped() {
        // 신버전 데몬이 레코드 꼬리에 필드를 더 붙여도 구버전 디코더가 읽는다
        let infos = sample();
        let mut bytes = encode_info(&infos[..1]);
        let len_at = 4;
        let len = u32::from_le_bytes(bytes[len_at..len_at + 4].try_into().unwrap());
        bytes.extend_from_slice(&[0xAA; 5]);
        bytes[len_at..len_at + 4].copy_from_slice(&(len + 5).to_le_bytes());
        assert_eq!(decode_info(&bytes), infos[..1].to_vec());
    }

    #[test]
    fn oversized_strings_are_clamped_to_u16() {
        let mut info = sample().remove(1);
        info.command = "x".repeat(70_000);
        let back = decode_info(&encode_info(std::slice::from_ref(&info)));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].command.len(), u16::MAX as usize);
    }

    #[test]
    fn session_id_needs_eight_bytes() {
        assert_eq!(session_id(&[1, 0, 0, 0, 0, 0, 0, 0, 9]), Some(1));
        assert_eq!(session_id(&[1, 0, 0]), None);
    }

    #[test]
    fn short_home_uses_the_cache_path() {
        let p = socket_path_for("/Users/me");
        assert_eq!(p, PathBuf::from("/Users/me/.cache/eden/mux/control.sock"));
    }

    #[test]
    fn long_home_falls_back_to_a_short_tmp_path() {
        // sun_path 한계(103자)를 넘는 HOME — 예전에는 bind/connect가 실패해
        // Session::new가 패닉했다.
        let home = format!("/tmp/{}", "x".repeat(120));
        let p = socket_path_for(&home);
        assert!(
            p.as_os_str().len() <= SUN_PATH_MAX,
            "폴백 경로도 sun_path 한계 안이어야 한다: {p:?}"
        );
        assert!(p.starts_with("/tmp"), "{p:?}");
    }

    #[test]
    fn different_long_homes_get_different_sockets() {
        // 폴백끼리도 격리 유지 — 같은 소켓을 쓰면 다른 HOME의 데몬이 섞인다.
        let a = socket_path_for(&format!("/tmp/{}/a", "x".repeat(120)));
        let b = socket_path_for(&format!("/tmp/{}/b", "x".repeat(120)));
        assert_ne!(a, b);
    }

    #[test]
    fn boundary_length_home_still_uses_the_cache_path() {
        // ".cache/eden/mux/control.sock" + "/" = 29자 → HOME 74자까지는 기본 경로.
        let home = format!("/{}", "h".repeat(73));
        let p = socket_path_for(&home);
        assert!(p.starts_with(&home));
        assert_eq!(p.as_os_str().len(), SUN_PATH_MAX);
    }
}
