//! AI: 자연어 → 셸 명령 생성 (Cmd+K), 실패한 명령 설명 (Cmd+Shift+K, 스트리밍).
//!
//! 로컬 우선 원칙: 계정/클라우드 강제 없음.
//! - `ANTHROPIC_API_KEY`가 있으면 Anthropic API (BYOK)
//! - 없으면 로컬 Ollama (`EDEN_OLLAMA_URL`, 기본 http://localhost:11434)
//!
//! 생성된 명령은 실행하지 않고 입력줄에 삽입만 한다 — 실행은 항상 사용자 몫.

use std::io::{BufRead, BufReader};

use serde_json::{Value, json};

const ANTHROPIC_MODEL: &str = "claude-opus-5-5";
/// 거절(`stop_reason: "refusal"`) 시 서버가 거절 분류에 맞는 다른 모델로 다시
/// 돌리게 한다. 셸 명령·에러 로그에는 보안 도구 이름이 흔해 분류기가 오판할
/// 여지가 있는데, 모델 이름을 박아 두지 않아도 되는 `"default"` 형식을 쓴다.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

const SYSTEM_PROMPT: &str = "\
You are a shell command generator embedded in a macOS terminal running zsh. \
The user describes what they want in natural language (Korean or English); \
you reply with EXACTLY ONE shell command that accomplishes it. \
Rules: reply with ONLY the command — no markdown code fences, no explanation, \
no leading `$`. Prefer safe, non-destructive commands; if the request implies \
deletion or overwriting, choose the most conservative form. If the request \
references the recent terminal output provided as context, use it.";

/// 명령 생성에 참고할 터미널 컨텍스트.
pub struct AiContext {
    /// 포커스된 페인의 최근 화면 텍스트 (마지막 수십 줄)
    pub screen_tail: String,
    /// 마지막으로 완료된 명령의 종료 코드
    pub last_exit: Option<i32>,
}

const EXPLAIN_PROMPT: &str = "\
You are embedded in a macOS terminal (zsh). A command just failed. Explain why it \
failed and how to fix it, for a developer who is looking at the same screen. \
Answer in the language the user writes in their commands' surrounding context; \
if unclear, answer in Korean. Be brief: at most 6 short lines of plain text, no \
markdown headings, no code fences. Lead with the cause in one sentence. \
If one shell command would fix or diagnose it, put it on the very last line as \
`FIX: <command>` — a single line, nothing after it. Prefer safe, non-destructive \
commands. If the output does not show enough to tell, say what to check instead \
of guessing.";

/// 실패한 명령 설명에 보낼 컨텍스트.
pub struct ExplainContext {
    /// 실행한 명령줄 (셸 통합이 안 보냈으면 None)
    pub command: Option<String>,
    pub exit: Option<i32>,
    /// 그 명령의 출력 (뒤쪽 위주로 잘라 둔 것)
    pub output: String,
    pub cwd: String,
}

/// 실패한 명령을 설명한다. 텍스트 조각이 올 때마다 `on_text`를 부른다
/// (스트리밍 — 긴 설명도 첫 글자가 바로 보이도록).
pub fn explain_failure(
    context: &ExplainContext,
    on_text: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let mut user = String::new();
    if !context.cwd.is_empty() {
        user.push_str(&format!("Working directory: {}\n", context.cwd));
    }
    match &context.command {
        Some(cmd) => user.push_str(&format!("Command: {cmd}\n")),
        None => user.push_str("Command: (unknown — shell integration did not report it)\n"),
    }
    if let Some(exit) = context.exit {
        user.push_str(&format!("Exit code: {exit}\n"));
    }
    user.push_str("Output (may be truncated at the start):\n---\n");
    user.push_str(&context.output);
    user.push_str("\n---");

    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        anthropic_stream(&key, EXPLAIN_PROMPT, &user, on_text)
    } else {
        ollama_stream(EXPLAIN_PROMPT, &user, on_text)
    }
}

/// 설명 끝의 `FIX: <명령>` 줄을 꺼낸다. 없으면 None.
pub fn fix_command(explanation: &str) -> Option<String> {
    let line = explanation.lines().rev().find(|l| !l.trim().is_empty())?;
    let cmd = line.trim().strip_prefix("FIX:")?.trim();
    let cmd = cmd.trim_matches('`').trim();
    (!cmd.is_empty()).then(|| cmd.to_string())
}

/// SSE `data:` 한 줄을 해석한 결과.
#[derive(Debug, PartialEq)]
enum SseEvent {
    Text(String),
    Refusal,
    Error(String),
    Other,
}

/// Messages API 스트림의 `data:` 줄 하나. 텍스트는 `content_block_delta`의
/// `text_delta`로만 온다 — 사고(thinking) 블록 등 다른 델타는 화면에 낼 게 없다.
fn parse_sse_data(data: &str) -> SseEvent {
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return SseEvent::Other;
    };
    match v["type"].as_str() {
        Some("content_block_delta") if v["delta"]["type"] == "text_delta" => {
            SseEvent::Text(v["delta"]["text"].as_str().unwrap_or_default().to_string())
        }
        Some("message_delta") if v["delta"]["stop_reason"] == "refusal" => SseEvent::Refusal,
        Some("error") => SseEvent::Error(
            v["error"]["message"]
                .as_str()
                .unwrap_or("알 수 없는 오류")
                .to_string(),
        ),
        _ => SseEvent::Other,
    }
}

fn anthropic_stream(
    api_key: &str,
    system: &str,
    user: &str,
    on_text: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let mut body = request_body(system, user, "low");
    body["stream"] = json!(true);
    let response = anthropic_post(api_key, &body)?;
    read_sse(BufReader::new(response.into_body().into_reader()), on_text)
}

/// SSE 본문을 줄 단위로 읽어 텍스트 조각을 넘긴다. `event:` 줄은 `data:`의
/// `type`과 같은 정보라 `data:`만 본다.
fn read_sse(reader: impl BufRead, on_text: &mut dyn FnMut(&str)) -> Result<(), String> {
    for line in reader.lines() {
        let line = line.map_err(|e| format!("스트림 읽기 실패: {e}"))?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        match parse_sse_data(data.trim_start()) {
            SseEvent::Text(text) => on_text(&text),
            SseEvent::Refusal => return Err("모델이 요청을 거절했습니다".to_string()),
            SseEvent::Error(msg) => return Err(format!("API 오류: {msg}")),
            SseEvent::Other => {}
        }
    }
    Ok(())
}

/// Ollama `/api/chat` 스트림 (줄마다 JSON 하나).
fn ollama_stream(system: &str, user: &str, on_text: &mut dyn FnMut(&str)) -> Result<(), String> {
    let (base, model) = ollama_target();
    let body = json!({
        "model": model,
        "stream": true,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    });
    let response = ureq::post(format!("{base}/api/chat"))
        .header("content-type", "application/json")
        .send(body.to_string())
        .map_err(|e| {
            format!("Ollama 요청 실패 ({e}). ANTHROPIC_API_KEY를 설정하거나 Ollama를 실행하세요")
        })?;
    read_ndjson(BufReader::new(response.into_body().into_reader()), on_text)
}

/// Ollama 스트림 본문 (줄마다 JSON 하나, `done: true`로 끝난다).
fn read_ndjson(reader: impl BufRead, on_text: &mut dyn FnMut(&str)) -> Result<(), String> {
    for line in reader.lines() {
        let line = line.map_err(|e| format!("스트림 읽기 실패: {e}"))?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(err) = v["error"].as_str() {
            return Err(format!("Ollama 오류: {err}"));
        }
        if let Some(text) = v["message"]["content"].as_str() {
            on_text(text);
        }
        if v["done"] == true {
            break;
        }
    }
    Ok(())
}

fn ollama_target() -> (String, String) {
    let base = std::env::var("EDEN_OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("EDEN_OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2".into());
    (base, model)
}

/// Messages API 요청 본문. 사고는 Opus 5.5에서 끌 수 없으므로 깊이는 `effort`로
/// 조절한다 — 명령 한 줄·짧은 설명이라 `low`면 충분하고 응답이 빠르다.
fn request_body(system: &str, user: &str, effort: &str) -> Value {
    json!({
        "model": ANTHROPIC_MODEL,
        "max_tokens": 16000,
        "system": system,
        "messages": [{"role": "user", "content": user}],
        "output_config": {"effort": effort},
        "fallbacks": "default",
    })
}

fn anthropic_post(api_key: &str, body: &Value) -> Result<ureq::http::Response<ureq::Body>, String> {
    ureq::post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", FALLBACK_BETA)
        .header("content-type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("Anthropic API 요청 실패: {e}"))
}

pub fn generate_command(request: &str, context: &AiContext) -> Result<String, String> {
    let mut user = String::new();
    if !context.screen_tail.trim().is_empty() {
        user.push_str("Recent terminal screen (most recent last):\n---\n");
        user.push_str(&context.screen_tail);
        user.push_str("\n---\n");
    }
    if let Some(exit) = context.last_exit {
        user.push_str(&format!("Last command exit code: {exit}\n"));
    }
    user.push_str("Request: ");
    user.push_str(request);

    let raw = if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        anthropic(&key, &user)?
    } else {
        ollama(&user)?
    };
    clean_command(&raw)
}

/// Anthropic Messages API (raw HTTP — Rust는 공식 SDK가 없어 cURL 형태를 따른다).
fn anthropic(api_key: &str, user: &str) -> Result<String, String> {
    let body = request_body(SYSTEM_PROMPT, user, "low");
    let mut response = anthropic_post(api_key, &body)?;

    let value: Value = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("응답 파싱 실패: {e}"))?;

    if value["stop_reason"] == "refusal" {
        return Err("모델이 요청을 거절했습니다".to_string());
    }

    let text: String = value["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if text.is_empty() {
        return Err("빈 응답".to_string());
    }
    Ok(text)
}

/// 로컬 Ollama (/api/chat).
fn ollama(user: &str) -> Result<String, String> {
    let (base, model) = ollama_target();

    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user},
        ],
    });

    let mut response = ureq::post(format!("{base}/api/chat"))
        .header("content-type", "application/json")
        .send(body.to_string())
        .map_err(|e| {
            format!("Ollama 요청 실패 ({e}). ANTHROPIC_API_KEY를 설정하거나 Ollama를 실행하세요")
        })?;

    let value: Value = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("응답 파싱 실패: {e}"))?;

    let text = value["message"]["content"].as_str().unwrap_or_default();
    if text.is_empty() {
        return Err("빈 응답".to_string());
    }
    Ok(text.to_string())
}

/// 모델 출력 정리: 코드 펜스/앞뒤 공백 제거, 명령만 남긴다.
fn clean_command(raw: &str) -> Result<String, String> {
    let mut text = raw.trim();

    // ```sh ... ``` 형태의 방어적 처리
    if text.starts_with("```") {
        let inner: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("```"))
            .collect();
        return clean_inner(&inner.join("\n"));
    }
    if let Some(stripped) = text.strip_prefix('$') {
        text = stripped.trim_start();
    }
    clean_inner(text)
}

fn clean_inner(text: &str) -> Result<String, String> {
    let cleaned = text.trim().to_string();
    if cleaned.is_empty() {
        Err("명령을 생성하지 못했습니다".to_string())
    } else {
        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_text_deltas_are_extracted() {
        let d = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"권한이"}}"#;
        assert_eq!(parse_sse_data(d), SseEvent::Text("권한이".into()));
    }

    #[test]
    fn sse_thinking_and_bookkeeping_events_are_ignored() {
        let thinking = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}"#;
        assert_eq!(parse_sse_data(thinking), SseEvent::Other);
        assert_eq!(
            parse_sse_data(r#"{"type":"message_stop"}"#),
            SseEvent::Other
        );
        assert_eq!(parse_sse_data(r#"{"type":"ping"}"#), SseEvent::Other);
        assert_eq!(parse_sse_data("not json"), SseEvent::Other);
        let end = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#;
        assert_eq!(parse_sse_data(end), SseEvent::Other);
    }

    #[test]
    fn sse_refusal_and_error_stop_the_stream() {
        let refusal = r#"{"type":"message_delta","delta":{"stop_reason":"refusal"}}"#;
        assert_eq!(parse_sse_data(refusal), SseEvent::Refusal);
        let err = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(parse_sse_data(err), SseEvent::Error("Overloaded".into()));
    }

    type Reader = fn(std::io::Cursor<&[u8]>, &mut dyn FnMut(&str)) -> Result<(), String>;

    fn collect(read: Reader, body: &str) -> (String, Result<(), String>) {
        let mut out = String::new();
        let result = read(std::io::Cursor::new(body.as_bytes()), &mut |t| {
            out.push_str(t)
        });
        (out, result)
    }

    #[test]
    fn sse_stream_concatenates_text_deltas_in_order() {
        let body = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"권한 \"}}\n\n\
event: ping\n\
data: {\"type\":\"ping\"}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"부족\\nFIX: ls\"}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";
        let (text, result) = collect(|r, f| read_sse(r, f), body);
        assert_eq!(result, Ok(()));
        assert_eq!(text, "권한 부족\nFIX: ls");
        assert_eq!(fix_command(&text), Some("ls".into()));
    }

    #[test]
    fn sse_stream_stops_on_error_event_keeping_earlier_text() {
        let body = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"앞부분\"}}\n\n\
event: error\n\
data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let (text, result) = collect(|r, f| read_sse(r, f), body);
        assert_eq!(text, "앞부분");
        assert_eq!(result, Err("API 오류: Overloaded".into()));
    }

    #[test]
    fn ollama_stream_reads_until_done() {
        let body = "{\"message\":{\"content\":\"원인: \"},\"done\":false}\n\
{\"message\":{\"content\":\"없음\"},\"done\":false}\n\
{\"done\":true}\n\
{\"message\":{\"content\":\"무시\"}}\n";
        let (text, result) = collect(|r, f| read_ndjson(r, f), body);
        assert_eq!(result, Ok(()));
        assert_eq!(text, "원인: 없음");
        let (_, err) = collect(
            |r, f| read_ndjson(r, f),
            "{\"error\":\"model not found\"}\n",
        );
        assert_eq!(err, Err("Ollama 오류: model not found".into()));
    }

    #[test]
    fn fix_line_is_taken_from_the_end_only() {
        let text = "권한이 없어 실패했습니다.\nFIX: chmod +x ./run.sh\n";
        assert_eq!(fix_command(text), Some("chmod +x ./run.sh".into()));
        assert_eq!(fix_command("FIX: `ls -la`"), Some("ls -la".into()));
        assert_eq!(fix_command("FIX: a\n원인은 위와 같습니다"), None);
        assert_eq!(fix_command("원인 불명"), None);
        assert_eq!(fix_command("FIX:   "), None);
    }
}
