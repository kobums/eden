//! AI 명령 생성: 자연어 → 셸 명령.
//!
//! 로컬 우선 원칙: 계정/클라우드 강제 없음.
//! - `ANTHROPIC_API_KEY`가 있으면 Anthropic API (BYOK)
//! - 없으면 로컬 Ollama (`EDEN_OLLAMA_URL`, 기본 http://localhost:11434)
//!
//! 생성된 명령은 실행하지 않고 입력줄에 삽입만 한다 — 실행은 항상 사용자 몫.

use serde_json::{Value, json};

const ANTHROPIC_MODEL: &str = "claude-opus-4-8";

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
    let body = json!({
        "model": ANTHROPIC_MODEL,
        "max_tokens": 1024,
        "system": SYSTEM_PROMPT,
        "messages": [{"role": "user", "content": user}],
    });

    let mut response = ureq::post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("Anthropic API 요청 실패: {e}"))?;

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
    let base = std::env::var("EDEN_OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("EDEN_OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2".into());

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
