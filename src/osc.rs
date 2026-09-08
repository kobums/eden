//! OSC(Operating System Command) 시퀀스 스캔 — GUI 세션과 mux 데몬이 공유한다.
//!
//! 셸 통합(OSC 133)과 작업 디렉터리(OSC 7)는 원래 GUI의 reader 스레드만
//! 봤지만, `eden list`가 GUI 없이도 "어느 세션이 명령 실행 중인가"를 알아야
//! 하므로 데몬도 같은 바이트를 같은 규칙으로 읽는다. 파서가 둘이면 어긋나기
//! 마련이라 여기 한 곳에 둔다. 전부 순수 함수다.

/// `ESC ] 133 ;` — 셸 통합 마크 프리픽스.
pub const OSC133_PREFIX: &[u8] = b"\x1b]133;";
/// `ESC ] 7 ;` — 작업 디렉터리 보고 프리픽스.
pub const OSC7_PREFIX: &[u8] = b"\x1b]7;";

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

/// OSC 133 페이로드 → 마크 종류.
///
/// 셸마다 형식이 조금씩 다르다. 첫 글자로만 분기하므로 뒤에 붙는 파라미터는
/// 무시된다 — 그래서 셋 다 그대로 동작한다:
///   zsh/bash(자체 스크립트) `A` `C;cmdline_url=…` `D;0`
///   fish(자체 내장, 3.4+)    `A;click_events=1` `C;cmdline_url=ls` `D;1`
pub fn parse_mark_kind(payload: &[u8]) -> Option<MarkKind> {
    match payload.first() {
        Some(b'A') => Some(MarkKind::PromptStart),
        Some(b'C') => Some(MarkKind::CommandStart),
        Some(b'D') => {
            let exit = payload
                .get(2..)
                .and_then(|s| std::str::from_utf8(s).ok())
                .and_then(|s| s.parse().ok());
            Some(MarkKind::CommandEnd(exit))
        }
        // B(프롬프트 끝) 등은 아직 사용하지 않음
        _ => None,
    }
}

/// `C;cmdline_url=<percent-encoded>` 에서 명령줄을 꺼낸다.
///
/// fish가 쓰는 파라미터 이름을 그대로 따랐다 — 우리 zsh/bash 스크립트도 같은
/// 이름으로 보낸다. 파라미터는 `;`로 구분되고, 명령줄 자체는 퍼센트 인코딩돼
/// 있어 `;`나 BEL이 섞일 수 없다. 없거나 UTF-8이 아니면 None.
pub fn parse_cmdline(payload: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(payload).ok()?;
    let mut params = s.split(';');
    if params.next()? != "C" {
        return None;
    }
    let raw = params.find_map(|p| p.strip_prefix("cmdline_url="))?;
    let decoded = String::from_utf8(percent_decode(raw.as_bytes())).ok()?;
    (!decoded.is_empty()).then_some(decoded)
}

/// `file://hostname/path` → `/path` (퍼센트 디코딩).
pub fn parse_osc7(payload: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(payload).ok()?;
    let rest = s.strip_prefix("file://")?;
    // 호스트 이후 첫 '/'부터가 경로
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => rest,
    };
    String::from_utf8(percent_decode(path.as_bytes())).ok()
}

/// `%XX` 퍼센트 디코딩. 짝이 맞지 않는 `%`는 문자 그대로 둔다.
pub fn percent_decode(bytes: &[u8]) -> Vec<u8> {
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
    out
}

/// 바이트 스트림에서 `prefix … BEL|ST` 페이로드를 전부 수집한다.
///
/// 청크 경계에 걸친 시퀀스는 `carry`에 이월하고, 종료 문자가 영영 안 오는
/// 스트림 때문에 carry가 무한히 크지 않게 상한을 둔다. 프리픽스마다 carry를
/// 따로 써야 한다 — 한 버퍼를 공유하면 한쪽 스캐너가 잘라낸 조각을 다른 쪽이
/// 놓친다.
pub fn collect_osc_payloads(carry: &mut Vec<u8>, chunk: &[u8], prefix: &[u8]) -> Vec<Vec<u8>> {
    carry.extend_from_slice(chunk);
    let mut out = Vec::new();

    loop {
        let Some(start) = find_subsequence(carry, prefix) else {
            // 프리픽스 없음: 경계에 걸린 프리픽스 후보만 남긴다
            let keep = longest_prefix_suffix(carry, prefix);
            let cut = carry.len() - keep;
            carry.drain(..cut);
            break;
        };
        let body = start + prefix.len();
        // 종료: BEL(0x07) 또는 ST(ESC \)
        let Some(rel) = carry[body..].iter().position(|&b| b == 0x07 || b == 0x1b) else {
            carry.drain(..start); // 종료 미도착: 프리픽스부터 보관하고 대기
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
        out.push(carry[body..term_pos].to_vec());
        carry.drain(..end);
    }

    // carry 무한 증가 방지
    if carry.len() > 8192 {
        let cut = carry.len() - 8192;
        carry.drain(..cut);
    }
    out
}

/// `haystack`에서 `needle`의 첫 위치.
pub fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `data`의 접미사 중 `pattern`의 진접두사인 최장 길이.
pub fn longest_prefix_suffix(data: &[u8], pattern: &[u8]) -> usize {
    let max = (pattern.len() - 1).min(data.len());
    for len in (1..=max).rev() {
        if data[data.len() - len..] == pattern[..len] {
            return len;
        }
    }
    0
}

/// 세션 하나의 셸 상태 — 데몬이 출력 바이트에서 도출한다.
///
/// GUI의 `Marks`(줄 번호가 붙은 전체 이력)와 달리 "지금 무엇을 하고 있나"만
/// 기억한다. `eden list`·`eden wait`가 보는 값이다.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellState {
    /// C 마크 이후 D 마크 전 — 명령이 실행 중이다.
    pub running: bool,
    /// 마지막 D 마크의 종료 코드.
    pub last_exit: Option<i32>,
    /// OSC 7으로 보고된 작업 디렉터리.
    pub cwd: String,
    /// 실행 중(또는 마지막으로 실행한) 명령줄. 셸 통합이 `cmdline_url`을
    /// 보내지 않으면 빈 문자열.
    pub command: String,
    /// 지금까지 시작된 명령 수 (C 마크 수). `eden send --wait`가 "내가 보낸
    /// 명령이 시작됐나"를 running 플래그의 순간값이 아니라 이 수의 증가로
    /// 판정한다 — 짧은 명령은 폴링 사이에 시작하고 끝나 버린다.
    pub commands: u64,
    /// 지금까지 끝난 명령 수 (D 마크 수). 위와 같은 이유로 완료 판정에 쓴다.
    pub completions: u64,
    osc133_carry: Vec<u8>,
    osc7_carry: Vec<u8>,
}

impl ShellState {
    /// 출력 청크 하나를 반영한다. 청크 경계는 신경 쓰지 않아도 된다.
    pub fn feed(&mut self, chunk: &[u8]) {
        for payload in collect_osc_payloads(&mut self.osc133_carry, chunk, OSC133_PREFIX) {
            match parse_mark_kind(&payload) {
                Some(MarkKind::CommandStart) => {
                    self.running = true;
                    self.commands += 1;
                    if let Some(cmd) = parse_cmdline(&payload) {
                        self.command = cmd;
                    }
                }
                Some(MarkKind::CommandEnd(exit)) => {
                    self.running = false;
                    self.completions += 1;
                    self.last_exit = exit;
                }
                // 프롬프트가 다시 그려지는 것은 상태 변화가 아니다.
                Some(MarkKind::PromptStart) | None => {}
            }
        }
        for payload in collect_osc_payloads(&mut self.osc7_carry, chunk, OSC7_PREFIX) {
            if let Some(path) = parse_osc7(&payload) {
                self.cwd = path;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_and_bash_marks() {
        assert_eq!(parse_mark_kind(b"A"), Some(MarkKind::PromptStart));
        assert_eq!(parse_mark_kind(b"C"), Some(MarkKind::CommandStart));
        assert_eq!(parse_mark_kind(b"D;0"), Some(MarkKind::CommandEnd(Some(0))));
        assert_eq!(
            parse_mark_kind(b"D;130"),
            Some(MarkKind::CommandEnd(Some(130))),
        );
    }

    #[test]
    fn fish_marks_with_parameters() {
        assert_eq!(
            parse_mark_kind(b"A;click_events=1"),
            Some(MarkKind::PromptStart)
        );
        assert_eq!(
            parse_mark_kind(b"C;cmdline_url=false"),
            Some(MarkKind::CommandStart)
        );
        assert_eq!(parse_mark_kind(b"D;1"), Some(MarkKind::CommandEnd(Some(1))));
    }

    #[test]
    fn prompt_end_and_unknown_marks_are_ignored() {
        assert_eq!(parse_mark_kind(b"B"), None, "B는 아직 쓰지 않는다");
        assert_eq!(parse_mark_kind(b"X"), None);
        assert_eq!(parse_mark_kind(b""), None);
    }

    #[test]
    fn missing_or_malformed_exit_code_becomes_none() {
        assert_eq!(parse_mark_kind(b"D"), Some(MarkKind::CommandEnd(None)));
        assert_eq!(parse_mark_kind(b"D;"), Some(MarkKind::CommandEnd(None)));
        assert_eq!(parse_mark_kind(b"D;abc"), Some(MarkKind::CommandEnd(None)));
    }

    #[test]
    fn cmdline_is_percent_decoded_from_the_c_mark() {
        assert_eq!(
            parse_cmdline(b"C;cmdline_url=git%20log%20--oneline"),
            Some("git log --oneline".into())
        );
        // 한글도 바이트 단위 인코딩이라 그대로 복원된다
        assert_eq!(
            parse_cmdline(b"C;cmdline_url=echo%20%ED%95%9C%EA%B8%80"),
            Some("echo 한글".into())
        );
        // 다른 파라미터가 앞에 있어도 찾는다
        assert_eq!(parse_cmdline(b"C;foo=1;cmdline_url=ls"), Some("ls".into()));
    }

    #[test]
    fn cmdline_is_absent_for_bare_or_foreign_marks() {
        assert_eq!(parse_cmdline(b"C"), None);
        assert_eq!(parse_cmdline(b"C;cmdline_url="), None);
        assert_eq!(parse_cmdline(b"A;cmdline_url=ls"), None);
        assert_eq!(parse_cmdline(b"D;0"), None);
    }

    #[test]
    fn osc7_strips_host_and_decodes_percent() {
        assert_eq!(
            parse_osc7(b"file://mac.local/Users/me/my%20dir"),
            Some("/Users/me/my dir".into())
        );
        assert_eq!(parse_osc7(b"http://x"), None);
    }

    #[test]
    fn percent_decode_keeps_dangling_percent() {
        assert_eq!(percent_decode(b"a%2"), b"a%2");
        assert_eq!(percent_decode(b"%"), b"%");
        assert_eq!(percent_decode(b"%zz"), b"%zz");
        assert_eq!(percent_decode(b"%41%42"), b"AB");
    }

    #[test]
    fn payloads_survive_chunk_boundaries() {
        let mut carry = Vec::new();
        let stream = b"x\x1b]133;D;0\x07y\x1b]133;A\x1b\\z";
        let mut got = Vec::new();
        // 한 바이트씩 흘려도 페이로드가 온전히 나온다
        for b in stream.iter() {
            got.extend(collect_osc_payloads(&mut carry, &[*b], OSC133_PREFIX));
        }
        assert_eq!(got, vec![b"D;0".to_vec(), b"A".to_vec()]);
        assert!(carry.is_empty());
    }

    #[test]
    fn unterminated_payload_waits_but_carry_is_bounded() {
        let mut carry = Vec::new();
        assert!(collect_osc_payloads(&mut carry, b"\x1b]133;C;cmdline", OSC133_PREFIX).is_empty());
        assert!(carry.starts_with(OSC133_PREFIX));
        let junk = vec![b'x'; 20_000];
        collect_osc_payloads(&mut carry, &junk, OSC133_PREFIX);
        assert!(carry.len() <= 8192);
    }

    #[test]
    fn shell_state_tracks_running_exit_cwd_and_command() {
        let mut s = ShellState::default();
        s.feed(b"\x1b]133;A\x07\x1b]7;file://h/Users/me\x07$ ");
        assert!(!s.running);
        assert_eq!(s.cwd, "/Users/me");

        s.feed(b"\x1b]133;C;cmdline_url=make%20test\x07building...");
        assert!(s.running);
        assert_eq!(s.command, "make test");
        assert_eq!(s.last_exit, None);

        s.feed(b"\x1b]133;D;2\x07\x1b]133;A\x07$ ");
        assert!(!s.running);
        assert_eq!(s.last_exit, Some(2));
        assert_eq!((s.commands, s.completions), (1, 1));
        // 명령줄은 마지막 것이 남는다 — "마지막에 뭘 돌렸나"가 유용하다
        assert_eq!(s.command, "make test");
    }

    #[test]
    fn shell_state_keeps_command_when_c_mark_has_no_cmdline() {
        let mut s = ShellState::default();
        s.feed(b"\x1b]133;C;cmdline_url=ls\x07\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;C\x07");
        assert!(s.running);
        // 파라미터 없는 C(구버전 스크립트·fish)는 명령줄을 지우지 않는다
        assert_eq!(s.command, "ls");
    }

    #[test]
    fn counters_see_a_command_that_started_and_ended_inside_one_chunk() {
        let mut s = ShellState::default();
        s.feed(b"\x1b]133;C;cmdline_url=false\x07\x1b]133;D;1\x07\x1b]133;A\x07");
        assert!(!s.running, "순간값으로는 아무 일도 없었던 것처럼 보인다");
        assert_eq!(
            (s.commands, s.completions),
            (1, 1),
            "카운터는 놓치지 않는다"
        );
    }

    #[test]
    fn shell_state_marks_split_across_chunks() {
        let mut s = ShellState::default();
        s.feed(b"\x1b]13");
        s.feed(b"3;C\x07");
        assert!(s.running);
        s.feed(b"\x1b]133;D;0");
        assert!(s.running, "종료 문자 전에는 아직 실행 중");
        s.feed(b"\x07");
        assert!(!s.running);
    }
}
