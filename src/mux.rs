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

// --- 프레임 태그 ---
// 클라이언트 → 데몬
const CREATE: u8 = 0x01; // cols u16, lines u16, px u16, py u16
const ATTACH: u8 = 0x02; // id u64
const INPUT: u8 = 0x03; // raw bytes
const RESIZE: u8 = 0x04; // cols u16, lines u16, px u16, py u16
const LIST: u8 = 0x05; // (empty)
const KILL: u8 = 0x06; // (empty) — 현재 세션 종료
// 데몬 → 클라이언트
const ATTACHED: u8 = 0x81; // id u64
const OUTPUT: u8 = 0x82; // raw bytes
const SESSION_LIST: u8 = 0x83; // count u32, ids u64..., boot u64 (꼬리 필드 — 구버전 데몬 응답에는 없다)
const EXIT: u8 = 0x85; // (empty) — 셸 종료

/// mux 제어 소켓 경로.
pub fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".cache/eden/mux/control.sock")
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
    std::process::Command::new(exe).arg("--daemon").spawn()?;

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
            let session = spawn_session(ws, id, Arc::clone(&registry));
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
                    session.lock().unwrap().pty.on_resize(ws);
                }
                serve_subscriber(stream, id, session);
            }
        }
        _ => {}
    }
}

/// PTY에 셸을 띄우고, 출력을 리플레이 버퍼 + 구독자에게 뿌리는 리더 스레드를 시작한다.
fn spawn_session(ws: WindowSize, id: u64, registry: Registry) -> Arc<Mutex<DaemonSession>> {
    // 데몬은 Dock에서 실행된 앱이 띄워 cwd가 `/`다. 지정하지 않으면
    // 셸이 그걸 상속해 `/`에서 시작하므로 홈 디렉터리를 명시한다.
    let mut options = tty::Options {
        working_directory: std::env::var("HOME").ok().map(PathBuf::from),
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
    let pty = tty::new(&options, ws, 0).expect("PTY 생성 실패");

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

    session
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
                    session.lock().unwrap().pty.on_resize(ws);
                }
            }
            Ok((KILL, _)) => {
                // 세션 종료: 셸에 SIGHUP을 보낸다. 리더 스레드가 EOF를 보고
                // 구독자에게 Ended를 알리고 레지스트리에서 제거한다.
                let pid = session.lock().unwrap().pty.child().id();
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGHUP);
                }
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
