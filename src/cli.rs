//! `eden <subcommand>` — GUI 없이 mux 데몬의 세션을 조작하는 CLI.
//!
//! tmux의 `list-windows` / `send-keys` / `capture-pane`에 해당한다. 스크립트나
//! AI 에이전트가 "세션을 만들고, 명령을 넣고, 끝나길 기다리고, 화면을 읽는"
//! 루프를 돌릴 수 있게 하는 것이 목적이다. 데몬이 이미 세션·PTY·리플레이
//! 버퍼·셸 상태를 갖고 있으므로 여기서는 소켓에 묻고 출력만 다듬는다.
//!
//! 인자 파싱은 손으로 한다 — 서브커맨드 여섯에 옵션 몇 개라 의존성을 더할
//! 이유가 없다. 알 수 없는 첫 인자는 GUI 실행으로 넘긴다 (Finder가 붙이는
//! 인자 등으로 앱이 안 뜨는 일이 없도록).

use std::io::{self, Write};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

use crate::mux::{self, SessionInfo};

/// 종료 코드. `wait`가 명령의 종료 코드를 그대로 돌려주므로 나머지는
/// 셸 관례에서 흔히 쓰지 않는 값으로 골랐다.
const EXIT_USAGE: i32 = 2;
const EXIT_NO_SESSION: i32 = 3;
const EXIT_TIMEOUT: i32 = 124;

const USAGE: &str = "\
eden — 세션 조작 CLI (GUI는 인자 없이 실행)

사용법:
  eden list [--json]                       살아있는 세션 목록
  eden new [--cwd DIR] [--json]            세션 생성 (붙지 않음; GUI가 다음 포커스 때 탭으로 붙인다)
  eden send <id> [옵션] <텍스트>...        세션에 입력 (기본: 끝에 Enter)
      -n, --no-enter                       Enter를 붙이지 않는다
      -r, --raw                            \\n \\r \\t \\e \\xHH \\\\ 이스케이프를 해석한다
      -w, --wait [--timeout SECS]          보낸 명령이 끝날 때까지 기다린다 (종료 코드 반환)
  eden wait <id> [--timeout SECS]          실행 중인 명령이 끝날 때까지 기다린다 (종료 코드 반환)
  eden capture <id> [-s] [-n N]            화면 텍스트 출력 (-s: 스크롤백 포함, -n: 마지막 N줄)
  eden kill <id>                           세션 종료 (셸에 SIGHUP)

종료 코드: 0 성공 · 1 오류 · 2 사용법 · 3 세션 없음 · 124 시간 초과
  (wait / send --wait 는 명령의 종료 코드를 그대로 돌려준다)
";

/// 서브커맨드면 실행하고 종료 코드를 돌려준다. GUI로 넘길 인자면 None.
pub fn run(args: &[String]) -> Option<i32> {
    let (cmd, rest) = args.split_first()?;
    let code = match cmd.as_str() {
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            0
        }
        "--version" | "-V" => {
            println!("eden {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "list" | "ls" => cmd_list(rest),
        "new" => cmd_new(rest),
        "send" => cmd_send(rest),
        "wait" => cmd_wait(rest),
        "capture" => cmd_capture(rest),
        "kill" => cmd_kill(rest),
        _ => return None,
    };
    Some(code)
}

fn usage_error(msg: &str) -> i32 {
    eprintln!("eden: {msg}\n\n{USAGE}");
    EXIT_USAGE
}

fn fail(err: impl std::fmt::Display) -> i32 {
    eprintln!("eden: {err}");
    1
}

fn parse_id(s: Option<&String>) -> Result<u64, i32> {
    match s.and_then(|s| s.parse().ok()) {
        Some(id) => Ok(id),
        None => Err(usage_error("세션 ID(숫자)가 필요합니다")),
    }
}

/// 데몬이 없으면 세션도 없다 — 오류가 아니라 빈 목록이다.
fn list_or_empty() -> io::Result<Vec<SessionInfo>> {
    match mux::list_info() {
        Err(e)
            if e.kind() == io::ErrorKind::NotFound
                || e.kind() == io::ErrorKind::ConnectionRefused =>
        {
            Ok(Vec::new())
        }
        other => other,
    }
}

// ---------------------------------------------------------------- list

fn cmd_list(args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    let infos = match list_or_empty() {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let out = if json {
        to_json(&infos)
    } else {
        format_table(&infos, &home)
    };
    print!("{out}");
    0
}

/// 사람용 표. 열 폭은 내용에 맞춘다 — 세션이 몇 개 안 되므로 두 번 훑어도 된다.
pub(crate) fn format_table(infos: &[SessionInfo], home: &str) -> String {
    if infos.is_empty() {
        return "세션 없음\n".to_string();
    }
    let rows: Vec<[String; 6]> = infos
        .iter()
        .map(|i| {
            [
                i.id.to_string(),
                if i.running { "running" } else { "idle" }.to_string(),
                i.last_exit.map_or("-".to_string(), |e| e.to_string()),
                format!("{}x{}", i.cols, i.lines),
                abbreviate_home(&i.cwd, home),
                i.command.clone(),
            ]
        })
        .collect();
    let header = ["ID", "STATE", "EXIT", "SIZE", "CWD", "COMMAND"];
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row.iter()) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    let line = |cells: &[&str], out: &mut String| {
        let mut row = String::new();
        for (cell, w) in cells.iter().zip(&widths) {
            row.push_str(cell);
            let pad = w - cell.chars().count() + 2;
            row.extend(std::iter::repeat_n(' ', pad));
        }
        // 빈 COMMAND 열이 줄 끝 공백으로 남지 않게
        out.push_str(row.trim_end());
        out.push('\n');
    };
    line(&header, &mut out);
    for row in &rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        line(&cells, &mut out);
    }
    out
}

/// `/Users/me/x` → `~/x`. 홈 자체는 `~`.
fn abbreviate_home(path: &str, home: &str) -> String {
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// 기계용 JSON 배열. serde_json은 이미 의존성에 있다.
pub(crate) fn to_json(infos: &[SessionInfo]) -> String {
    let items: Vec<serde_json::Value> = infos.iter().map(info_json).collect();
    let mut s = serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into());
    s.push('\n');
    s
}

fn info_json(i: &SessionInfo) -> serde_json::Value {
    serde_json::json!({
        "id": i.id,
        "pid": i.pid,
        "cols": i.cols,
        "lines": i.lines,
        "running": i.running,
        "last_exit": i.last_exit,
        "cwd": i.cwd,
        "command": i.command,
        "commands": i.commands,
        "completions": i.completions,
    })
}

// ---------------------------------------------------------------- new

fn cmd_new(args: &[String]) -> i32 {
    let mut cwd: Option<String> = None;
    let mut json = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--cwd" => match it.next() {
                Some(dir) => cwd = Some(dir.clone()),
                None => return usage_error("--cwd 뒤에 디렉터리가 필요합니다"),
            },
            other => return usage_error(&format!("알 수 없는 옵션: {other}")),
        }
    }
    // 기본은 CLI를 실행한 디렉터리 — tmux new-window가 현재 페인의 cwd를
    // 따르는 것과 같은 기대. 데몬 기본값(홈)은 GUI의 새 탭에만 해당한다.
    let dir = match cwd {
        Some(d) => std::path::PathBuf::from(d),
        None => match std::env::current_dir() {
            Ok(d) => d,
            Err(e) => return fail(e),
        },
    };
    let dir = match dir.canonicalize() {
        Ok(d) if d.is_dir() => d,
        _ => return fail(format!("디렉터리가 없습니다: {}", dir.display())),
    };
    if let Err(e) = mux::ensure_daemon() {
        return fail(e);
    }
    // 크기는 자리표시자다. GUI가 붙을 때 실제 페인 크기로 리사이즈한다.
    let ws = WindowSize {
        num_cols: 80,
        num_lines: 24,
        cell_width: 8,
        cell_height: 16,
    };
    match mux::spawn(ws, dir.to_str()) {
        Ok(id) => {
            if json {
                println!("{{\"id\": {id}}}");
            } else {
                println!("{id}");
            }
            0
        }
        Err(e) => fail(e),
    }
}

// ---------------------------------------------------------------- send / wait

fn cmd_send(args: &[String]) -> i32 {
    let mut it = args.iter().peekable();
    let id = match parse_id(it.next()) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let mut enter = true;
    let mut raw = false;
    let mut wait = false;
    let mut timeout: Option<Duration> = None;
    let mut text: Vec<&str> = Vec::new();
    while let Some(a) = it.next() {
        // 옵션은 텍스트 앞뒤 어디에 와도 된다. 모르는 `-x`는 텍스트다 —
        // `eden send 1 ls -la`의 `-la`를 옵션으로 오해하지 않기 위해서고,
        // 텍스트에 진짜 `--wait`를 넣고 싶으면 `--` 뒤에 쓴다.
        match a.as_str() {
            "-n" | "--no-enter" => enter = false,
            "-r" | "--raw" => raw = true,
            "-w" | "--wait" => wait = true,
            "--timeout" => match it.next().and_then(|s| s.parse::<f64>().ok()) {
                Some(secs) => timeout = Some(Duration::from_secs_f64(secs)),
                None => return usage_error("--timeout 뒤에 초(숫자)가 필요합니다"),
            },
            "--" => {
                text.extend(it.by_ref().map(String::as_str));
            }
            _ => text.push(a),
        }
    }
    let joined = text.join(" ");
    let mut bytes = if raw {
        unescape(&joined)
    } else {
        joined.into_bytes()
    };
    if enter {
        // CR — raw 모드 TUI(Claude Code 등)는 Enter를 CR로 받고, 쿡드 모드
        // 셸은 tty가 CR→NL로 바꿔 주므로 양쪽 다 통한다.
        bytes.push(b'\r');
    }
    // --wait는 보내기 전의 카운터를 기준점으로 잡는다. running 플래그의
    // 순간값을 폴링하면 `false`처럼 짧은 명령은 시작과 끝을 둘 다 놓친다.
    let before = if wait {
        match find_session(id) {
            Ok(Some(info)) => Some(info),
            Ok(None) => return no_session(id),
            Err(e) => return fail(e),
        }
    } else {
        None
    };
    match mux::send(id, &bytes) {
        Ok(true) => {}
        Ok(false) => return no_session(id),
        Err(e) => return fail(e),
    }
    let Some(before) = before else { return 0 };
    // 보낸 입력이 명령이 아닐 수도 있다 (빈 줄, TUI 앱 내부 입력). 잠깐
    // 기다려도 명령이 시작되지 않으면 기다릴 것이 없으므로 0.
    let started = Instant::now();
    let grace = Duration::from_secs(2);
    loop {
        match find_session(id) {
            Ok(Some(info)) if info.completions > before.completions => {
                return info.last_exit.unwrap_or(0);
            }
            Ok(Some(info)) if info.commands == before.commands && started.elapsed() >= grace => {
                return 0;
            }
            Ok(Some(_)) => {}
            Ok(None) => return no_session(id),
            Err(e) => return fail(e),
        }
        if timeout.is_some_and(|t| started.elapsed() >= t) {
            eprintln!("eden: 세션 {id}의 명령이 제한 시간 안에 끝나지 않았습니다");
            return EXIT_TIMEOUT;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn find_session(id: u64) -> io::Result<Option<SessionInfo>> {
    mux::list_info().map(|v| v.into_iter().find(|i| i.id == id))
}

fn cmd_wait(args: &[String]) -> i32 {
    let mut it = args.iter();
    let id = match parse_id(it.next()) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let mut timeout = None;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--timeout" => match it.next().and_then(|s| s.parse::<f64>().ok()) {
                Some(secs) => timeout = Some(Duration::from_secs_f64(secs)),
                None => return usage_error("--timeout 뒤에 초(숫자)가 필요합니다"),
            },
            other => return usage_error(&format!("알 수 없는 옵션: {other}")),
        }
    }
    wait_idle(id, timeout)
}

/// 세션의 명령이 끝날 때까지 폴링. 종료 코드를 프로세스 종료 코드로 돌려준다.
fn wait_idle(id: u64, timeout: Option<Duration>) -> i32 {
    let started = Instant::now();
    loop {
        match find_session(id) {
            Ok(Some(info)) if !info.running => return info.last_exit.unwrap_or(0),
            Ok(Some(_)) => {}
            Ok(None) => return no_session(id),
            Err(e) => return fail(e),
        }
        if timeout.is_some_and(|t| started.elapsed() >= t) {
            eprintln!("eden: 세션 {id}의 명령이 제한 시간 안에 끝나지 않았습니다");
            return EXIT_TIMEOUT;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn no_session(id: u64) -> i32 {
    eprintln!("eden: 세션 {id}이(가) 없습니다");
    EXIT_NO_SESSION
}

/// `--raw` 이스케이프 해석. 모르는 이스케이프는 문자 그대로 둔다.
pub(crate) fn unescape(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let esc = bytes[i + 1];
        let simple = match esc {
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'e' => Some(0x1b),
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'0' => Some(0),
            b'\\' => Some(b'\\'),
            _ => None,
        };
        if let Some(b) = simple {
            out.push(b);
            i += 2;
            continue;
        }
        if esc == b'x'
            && let Some(hex) = bytes.get(i + 2..i + 4)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 4;
            continue;
        }
        out.push(b'\\');
        i += 1;
    }
    out
}

// ---------------------------------------------------------------- capture

fn cmd_capture(args: &[String]) -> i32 {
    let mut it = args.iter();
    let id = match parse_id(it.next()) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let mut scrollback = false;
    let mut last: Option<usize> = None;
    while let Some(a) = it.next() {
        match a.as_str() {
            "-s" | "--scrollback" => scrollback = true,
            "-n" | "--lines" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => last = Some(n),
                None => return usage_error("-n 뒤에 줄 수가 필요합니다"),
            },
            other => return usage_error(&format!("알 수 없는 옵션: {other}")),
        }
    }
    let (cols, lines, replay) = match mux::peek(id) {
        Ok(Some(v)) => v,
        Ok(None) => return no_session(id),
        Err(e) => return fail(e),
    };
    let text = render_replay(cols, lines, &replay, scrollback, last);
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
    0
}

/// 리플레이 바이트를 세션과 같은 크기의 헤드리스 Term에 재생해 텍스트로 뽑는다.
///
/// GUI가 붙을 때 하는 일과 같다 — 그래서 GUI가 보는 화면과 같은 결과가
/// 나온다. 리플레이가 2MB에서 잘려 있으면 앞부분 스크롤백은 없다.
pub(crate) fn render_replay(
    cols: u16,
    lines: u16,
    replay: &[u8],
    scrollback: bool,
    last: Option<usize>,
) -> String {
    struct Nop;
    impl EventListener for Nop {}

    let size = Size {
        columns: cols.max(1) as usize,
        lines: lines.max(1) as usize,
    };
    let config = Config {
        scrolling_history: 10_000,
        ..Config::default()
    };
    let mut term = Term::new(config, &size, Nop);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, replay);

    let grid = term.grid();
    let top = if scrollback {
        -(grid.history_size() as i32)
    } else {
        0
    };
    let bottom = grid.screen_lines() as i32 - 1;
    let text = term.bounds_to_string(
        Point::new(Line(top), Column(0)),
        Point::new(Line(bottom), Column(grid.columns() - 1)),
    );
    // 화면 아래쪽 빈 줄은 정보가 아니다
    let mut rows: Vec<&str> = text.lines().collect();
    while rows.last().is_some_and(|l| l.trim().is_empty()) {
        rows.pop();
    }
    if let Some(n) = last
        && rows.len() > n
    {
        rows.drain(..rows.len() - n);
    }
    let mut out = rows.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// 헤드리스 Term의 크기. session.rs의 `TermSize`와 같지만 그쪽은 GUI 모듈에
/// 묶여 있어 여기서 따로 둔다.
struct Size {
    columns: usize,
    lines: usize,
}

impl Dimensions for Size {
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

// ---------------------------------------------------------------- kill

fn cmd_kill(args: &[String]) -> i32 {
    let id = match parse_id(args.first()) {
        Ok(id) => id,
        Err(code) => return code,
    };
    match mux::kill_id(id) {
        Ok(true) => 0,
        Ok(false) => no_session(id),
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: u64, running: bool, exit: Option<i32>, cwd: &str, cmd: &str) -> SessionInfo {
        SessionInfo {
            id,
            pid: 100 + id as u32,
            cols: 80,
            lines: 24,
            running,
            last_exit: exit,
            cwd: cwd.into(),
            command: cmd.into(),
            commands: 0,
            completions: 0,
        }
    }

    #[test]
    fn unknown_first_arg_falls_through_to_gui() {
        assert_eq!(run(&[]), None);
        assert_eq!(run(&["-psn_0_12345".to_string()]), None);
        assert_eq!(run(&["/some/file.txt".to_string()]), None);
    }

    #[test]
    fn table_aligns_columns_and_abbreviates_home() {
        let infos = [
            info(1, true, None, "/Users/me/develop/eden", "cargo test"),
            info(12, false, Some(130), "/Users/me", ""),
            info(3, false, Some(0), "/tmp", "ls"),
        ];
        let table = format_table(&infos, "/Users/me");
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("ID  STATE    EXIT  SIZE   CWD"));
        assert!(lines[1].starts_with("1   running  -     80x24  ~/develop/eden"));
        assert!(lines[1].ends_with("cargo test"));
        assert!(lines[2].starts_with("12  idle     130   80x24  ~"));
        // 빈 COMMAND 열 때문에 줄 끝 공백이 생기지 않는다
        assert_eq!(lines[2], lines[2].trim_end());
        assert!(lines[3].contains("/tmp"));
    }

    #[test]
    fn empty_list_says_so() {
        assert_eq!(format_table(&[], "/Users/me"), "세션 없음\n");
    }

    #[test]
    fn home_abbreviation_only_matches_path_boundary() {
        assert_eq!(abbreviate_home("/Users/me", "/Users/me"), "~");
        assert_eq!(abbreviate_home("/Users/me/x", "/Users/me"), "~/x");
        assert_eq!(abbreviate_home("/Users/meow", "/Users/me"), "/Users/meow");
        assert_eq!(abbreviate_home("/tmp", ""), "/tmp");
    }

    #[test]
    fn json_output_is_an_array_with_nullable_exit() {
        let s = to_json(&[info(1, true, None, "/x", "make")]);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v[0]["id"], 1);
        assert_eq!(v[0]["running"], true);
        assert!(v[0]["last_exit"].is_null());
        assert_eq!(v[0]["command"], "make");
        assert_eq!(to_json(&[]).trim(), "[]");
    }

    #[test]
    fn unescape_handles_common_and_hex_escapes() {
        assert_eq!(unescape(r"a\nb\r\t\e\\"), b"a\nb\r\t\x1b\\");
        assert_eq!(unescape(r"\x03"), vec![3]);
        assert_eq!(unescape(r"\x1b[A"), b"\x1b[A");
        // 모르는 이스케이프·불완전한 \x는 그대로
        assert_eq!(unescape(r"\q"), b"\\q");
        assert_eq!(unescape(r"\x4"), b"\\x4");
        assert_eq!(unescape(r"end\"), b"end\\");
        // 한글은 그대로 지나간다
        assert_eq!(unescape("한글\\n"), "한글\n".as_bytes());
    }

    #[test]
    fn render_replay_reproduces_the_screen() {
        let replay = b"$ echo hi\r\nhi\r\n$ ";
        assert_eq!(
            render_replay(20, 5, replay, false, None),
            "$ echo hi\nhi\n$\n"
        );
    }

    #[test]
    fn render_replay_respects_scrollback_and_last_n() {
        // 5줄 화면에 8줄을 찍으면 3줄이 스크롤백으로 밀린다
        let mut replay = Vec::new();
        for i in 1..=8 {
            replay.extend_from_slice(format!("line{i}\r\n").as_bytes());
        }
        let screen = render_replay(20, 5, &replay, false, None);
        assert_eq!(
            screen, "line5\nline6\nline7\nline8\n",
            "화면에는 마지막 4줄 + 빈 커서 줄"
        );
        let all = render_replay(20, 5, &replay, true, None);
        assert!(all.starts_with("line1\nline2\n"));
        assert_eq!(all.lines().count(), 8);
        assert_eq!(
            render_replay(20, 5, &replay, true, Some(2)),
            "line7\nline8\n"
        );
        assert_eq!(render_replay(20, 5, b"", false, None), "");
    }

    #[test]
    fn render_replay_ignores_shell_integration_sequences() {
        let replay = b"\x1b]133;A\x07\x1b]7;file://h/tmp\x07$ \x1b]133;C;cmdline_url=ls\x07\r\nout\r\n\x1b]133;D;0\x07";
        assert_eq!(render_replay(20, 5, replay, false, None), "$\nout\n");
    }
}
