//! 하단 상태바 문자열: cwd(OSC 7) + git 브랜치 / CPU · 메모리 · 시계.

use super::App;

/// git 브랜치 아이콘 (powerline). 폰트에 없으면 렌더러가 조용히 건너뛴다.
const GIT_BRANCH_ICON: char = '\u{E0A0}';
const FOLDER_ICON: char = '\u{1F4C1}';

impl App {
    /// 포커스된 페인 기준 상태바 문자열 (왼쪽, 오른쪽).
    pub(super) fn status_strings(&self) -> (String, String) {
        let state = self.state.as_ref().unwrap();
        let cwd = state.focused_pane().session.cwd();

        let mut left = format!(" {FOLDER_ICON} {}", display_cwd(&cwd));
        if let Some(branch) = git_branch(&cwd) {
            left.push_str(&format!("   {GIT_BRANCH_ICON} {branch}"));
        }

        let cpu = self.sys.global_cpu_usage();
        let mem_used = self.sys.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let mem_total = self.sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let right = format!(
            "CPU {cpu:.0}%   MEM {mem_used:.1}/{mem_total:.0}G   {}  ",
            clock_hms()
        );
        (left, right)
    }
}

/// 홈 디렉터리는 `~`로 축약한다. cwd를 아직 모르면 `—`.
fn display_cwd(cwd: &str) -> String {
    if cwd.is_empty() {
        return "—".to_string();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    match cwd.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => cwd.to_string(),
    }
}

/// cwd에서 상위로 올라가며 `.git/HEAD`를 찾아 현재 브랜치명을 돌려준다.
/// detached면 짧은 커밋 해시.
fn git_branch(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let mut dir = std::path::PathBuf::from(cwd);
    loop {
        let head = dir.join(".git/HEAD");
        if let Ok(content) = std::fs::read_to_string(&head) {
            let content = content.trim();
            if let Some(rest) = content.strip_prefix("ref: refs/heads/") {
                return Some(rest.to_string());
            }
            // detached HEAD: 짧은 해시
            return Some(content.chars().take(7).collect());
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// 로컬 시간 HH:MM:SS (libc localtime, 추가 의존성 없이).
fn clock_hms() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
