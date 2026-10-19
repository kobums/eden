//! 패닉 로그 — `~/.cache/eden/panic.log`.
//!
//! `.app`으로 실행하면 stderr가 어디에도 남지 않아, 패닉이 나도 "창이 그냥
//! 사라졌다"는 증상만 남는다. Phase 17의 알림 배너 크래시가 수동 검증에서만
//! 잡혔던 것도 그래서다. 기본 훅(stderr 출력)은 그대로 두고, 그 앞에 파일
//! 기록을 끼운다. GUI·데몬·CLI가 같은 파일에 쓰므로 줄마다 역할과 pid를 붙인다.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 이 크기를 넘으면 `panic.log.1`로 한 번 돌린다 (이전 `.1`은 버린다)
const MAX_BYTES: u64 = 1_000_000;

/// `role`: "gui" / "daemon" / "cli". 프로세스 시작 직후 한 번 부른다.
pub fn install(role: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = log_path() {
            let thread = std::thread::current();
            let entry = format_entry(
                &timestamp(),
                role,
                std::process::id(),
                thread.name().unwrap_or("<unnamed>"),
                &info.to_string(),
                &std::backtrace::Backtrace::force_capture().to_string(),
            );
            // 훅 안에서 다시 패닉하면 프로세스가 abort되므로 실패는 전부 삼킨다
            let _ = append(&path, &entry);
        }
        previous(info);
    }));
}

fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".cache/eden/panic.log"))
}

/// 한 건의 기록. 순수 함수라 테스트에서 형식을 고정한다.
fn format_entry(
    time: &str,
    role: &str,
    pid: u32,
    thread: &str,
    message: &str,
    backtrace: &str,
) -> String {
    format!(
        "=== {time} eden {version} {role} pid={pid} thread={thread}\n{message}\n{backtrace}\n",
        version = env!("CARGO_PKG_VERSION"),
        backtrace = backtrace.trim_end(),
    )
}

fn append(path: &Path, entry: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(entry.as_bytes())
}

/// 로컬 시각 "2026-10-19 13:40:05". 날짜 크레이트 없이 libc로 만든다.
fn timestamp() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return format!("unix:{now}");
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_has_header_message_and_backtrace() {
        let entry = format_entry(
            "2026-10-19 13:40:05",
            "gui",
            42,
            "main",
            "panicked at src/app/mod.rs:10:5:\nboom",
            "   0: eden::main\n\n",
        );
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            entry,
            format!(
                "=== 2026-10-19 13:40:05 eden {version} gui pid=42 thread=main\n\
                 panicked at src/app/mod.rs:10:5:\nboom\n   0: eden::main\n"
            )
        );
    }

    #[test]
    fn append_creates_dir_and_rotates_when_large() {
        let dir = std::env::temp_dir().join(format!("eden-panic-test-{}", std::process::id()));
        let path = dir.join("nested/panic.log");
        let _ = fs::remove_dir_all(&dir);

        append(&path, "first\n").unwrap();
        append(&path, "second\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");

        // 상한을 넘긴 파일은 다음 기록 전에 .1로 밀려난다
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        append(&path, "third\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "third\n");
        assert!(path.with_extension("log.1").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timestamp_is_local_datetime() {
        let t = timestamp();
        assert_eq!(t.len(), 19, "{t}");
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], " ");
    }
}
