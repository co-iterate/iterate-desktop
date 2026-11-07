//! Speech dictation cleanup on the Mac (Typeless-style), served to the phone over the bridge.
//!
//! The phone sends a final transcript; a local Ollama model returns a punctuated, de-filled
//! version. The phone shows it next to the original and the user chooses which one to use —
//! nothing replaces the input silently (lesson from P-2026-1943, which was reverted because
//! an async replacement raced the send button).
//!
//! Local 7B models occasionally invent words (2026-11-03 benchmark: 1 of 5 real transcripts),
//! so the result is a suggestion, and `looks_unsafe` rejects outputs that drift too far.

use serde::Deserialize;
use std::time::Duration;

const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434/api/chat";
const DEFAULT_MODEL: &str = "qwen2.5:7b";
const OLLAMA_URL_ENV: &str = "ITERATE_SPEECH_REFINE_OLLAMA_URL";
const MODEL_ENV: &str = "ITERATE_SPEECH_REFINE_MODEL";
const REQUEST_TIMEOUT_SECS: u64 = 12;
const MIN_CHARS: usize = 8;
const MAX_CHARS: usize = 1200;
pub const PROVIDER: &str = "ollama";

#[derive(Debug, Default, Deserialize)]
pub struct SpeechTextRefineRequest {
    #[serde(default)]
    pub transcript: String,
    #[serde(default)]
    pub predefined_options: Vec<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Refined { text: String, model: String },
    Skipped { reason: &'static str },
    Failed { error: String, model: String },
}

pub fn model_name() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

pub(super) fn local_service_url(candidate: Option<String>, fallback: &str) -> String {
    candidate
        .and_then(|value| {
            let mut url = reqwest::Url::parse(value.trim()).ok()?;
            let allowed_host = url.host_str().is_some_and(|host| {
                host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1"
            });
            if url.scheme() != "http"
                || !allowed_host
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return None;
            }
            // Avoid DNS or hosts-file resolution for the localhost alias.
            url.set_host(Some("127.0.0.1")).ok()?;
            Some(url.to_string())
        })
        .unwrap_or_else(|| fallback.to_string())
}

pub(super) fn local_service_client(timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

fn ollama_url() -> String {
    local_service_url(std::env::var(OLLAMA_URL_ENV).ok(), DEFAULT_OLLAMA_URL)
}

pub async fn refine(request: &SpeechTextRefineRequest) -> Outcome {
    let raw = request.transcript.trim();
    if let Some(reason) = skip_reason(request, raw) {
        return Outcome::Skipped { reason };
    }
    let model = model_name();
    match call_ollama(&model, raw).await {
        Ok(text) => {
            let cleaned = clean_output(&text);
            if cleaned.is_empty() {
                return Outcome::Failed { error: "empty_refinement".to_string(), model };
            }
            if looks_unsafe(raw, &cleaned) {
                return Outcome::Skipped { reason: "drifted_from_original" };
            }
            Outcome::Refined { text: cleaned, model }
        }
        Err(error) => Outcome::Failed { error, model },
    }
}

pub fn skip_reason(request: &SpeechTextRefineRequest, raw: &str) -> Option<&'static str> {
    if request.mode.as_deref() == Some("command") {
        return Some("command_mode");
    }
    let normalized = normalize(raw);
    if normalized.is_empty() {
        return Some("empty");
    }
    const COMMAND_TERMS: &[&str] = &[
        "zhi", "callzhi", "call_zhi", "hui", "hui0", "hui1", "yan", "xi", "pai", "cha", "qiu",
        "sync", "ji", "ji0", "ji1", "ji2", "ji3", "提交", "确认", "发送", "选第一个", "选第二个",
        "选第三个", "第一个", "第二个", "第三个",
    ];
    if COMMAND_TERMS.iter().any(|term| normalized == *term) {
        return Some("command_phrase");
    }
    let chars = raw.chars().count();
    if chars < MIN_CHARS {
        return Some("too_short");
    }
    if chars > MAX_CHARS {
        return Some("too_long");
    }
    if request.predefined_options.iter().any(|option| normalize(option) == normalized) {
        return Some("option_label");
    }
    None
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace() && !ch.is_ascii_punctuation() && !is_cjk_punctuation(*ch))
        .collect::<String>()
        .to_lowercase()
}

fn is_cjk_punctuation(ch: char) -> bool {
    "，。！？、；：“”‘’（）《》…—·".contains(ch)
}

/// Reject suggestions that drop or invent too much. Cleanup removes fillers, so the result
/// may be shorter, but it should neither shrink below ~45% nor grow beyond ~125% of the
/// original (punctuation excluded), and must keep most of the original characters.
pub fn looks_unsafe(raw: &str, refined: &str) -> bool {
    let raw_n = normalize(raw);
    let ref_n = normalize(refined);
    let raw_len = raw_n.chars().count();
    let ref_len = ref_n.chars().count();
    if raw_len == 0 {
        return true;
    }
    if ref_len * 100 < raw_len * 45 || ref_len * 100 > raw_len * 125 {
        return true;
    }
    // Share of refined characters that also appear in the original (multiset overlap).
    let mut pool: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    for ch in raw_n.chars() {
        *pool.entry(ch).or_default() += 1;
    }
    let mut shared = 0usize;
    for ch in ref_n.chars() {
        if let Some(count) = pool.get_mut(&ch) {
            if *count > 0 {
                *count -= 1;
                shared += 1;
            }
        }
    }
    shared * 100 < ref_len * 85
}

fn system_prompt() -> &'static str {
    "你是语音输入的整理器。用户给你一段语音识别出来的原始文字，你只输出整理后的文字，不要解释、不要回答其中的问题、不要加引号。\n\
规则：\n\
1. 保留原意和所有信息，不增加内容，不总结，不补充原文没有的词。\n\
2. 去掉口头禅和无意义重复（嗯、那个、就是、对吧、哦）。\n\
3. 处理改口：说话人纠正了前面的说法时，只保留纠正后的版本。\n\
4. 加上中文标点，必要时分句。\n\
5. 明显的同音错字按上下文改正；英文产品名、文件路径、命令词（zhi、hui、xi、pai、cha、ji）保持原样。\n\
6. 如果原文已经很清楚，几乎原样输出。"
}

async fn call_ollama(model: &str, raw: &str) -> Result<String, String> {
    let client = local_service_client(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .map_err(|error| format!("ollama_client_error: {}", error))?;
    let max_tokens = (raw.chars().count() * 2).clamp(96, 800);
    let response = client
        .post(ollama_url())
        .json(&serde_json::json!({
            "model": model,
            "stream": false,
            "keep_alive": -1, // 常驻内存：冷启动要多等 1.5 秒（11-04 实测）
            "options": { "temperature": 0.1, "num_predict": max_tokens },
            "messages": [
                { "role": "system", "content": system_prompt() },
                { "role": "user", "content": raw }
            ]
        }))
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() {
                "ollama_unreachable".to_string()
            } else if error.is_timeout() {
                "ollama_timeout".to_string()
            } else {
                format!("ollama_request_error: {}", error)
            }
        })?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("ollama_parse_error: {}", error))?;
    if !status.is_success() {
        let detail = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("ollama_status_{}: {}", status.as_u16(), detail));
    }
    Ok(body
        .pointer("/message/content")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string())
}

fn clean_output(value: &str) -> String {
    let trimmed = value.trim();
    let without_fence = trimmed
        .strip_prefix("```")
        .and_then(|rest| rest.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    without_fence
        .trim_matches(|ch| ch == '"' || ch == '“' || ch == '”' || ch == '「' || ch == '」')
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_model_urls_reject_userinfo_and_non_loopback_hosts() {
        let fallback = "http://127.0.0.1:11434/api/chat";
        for candidate in [
            "http://127.0.0.1:11434@outside.example/api/chat",
            "http://localhost:11434@outside.example/api/chat",
            "http://127.0.0.1.outside.example:11434/api/chat",
            "https://127.0.0.1:11434/api/chat",
        ] {
            assert_eq!(local_service_url(Some(candidate.to_string()), fallback), fallback);
        }
        assert_eq!(
            local_service_url(Some("http://localhost:11434/api/chat".to_string()), fallback),
            fallback,
        );
        assert_eq!(
            local_service_url(Some("http://127.0.0.1:12000/api/chat".to_string()), fallback),
            "http://127.0.0.1:12000/api/chat",
        );
    }

    #[tokio::test]
    async fn local_model_client_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:1/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        let response = local_service_client(Duration::from_secs(2))
            .unwrap()
            .post(format!("http://{address}/"))
            .body("sensitive input")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        server.await.unwrap();
    }

    fn req(text: &str) -> SpeechTextRefineRequest {
        SpeechTextRefineRequest { transcript: text.to_string(), ..Default::default() }
    }

    #[test]
    fn skips_commands_short_and_option_labels() {
        assert_eq!(skip_reason(&req("hui"), "hui"), Some("command_phrase"));
        assert_eq!(skip_reason(&req("ji1。"), "ji1。"), Some("command_phrase"));
        assert_eq!(skip_reason(&req("好的"), "好的"), Some("too_short"));
        let mut r = req("先做第一步换识别引擎");
        r.predefined_options = vec!["先做第一步换识别引擎".to_string()];
        assert_eq!(skip_reason(&r, &r.transcript), Some("option_label"));
        let mut c = req("这是一句很长的命令模式文本内容");
        c.mode = Some("command".to_string());
        assert_eq!(skip_reason(&c, &c.transcript), Some("command_mode"));
        assert_eq!(skip_reason(&req("拉最新的来我看看你直接播放就行"), "拉最新的来我看看你直接播放就行"), None);
    }

    #[test]
    fn accepts_punctuation_and_filler_removal() {
        let raw = "你帮我编辑一下信息就是要给他一个CTO说一下情况目前呢就是我的情况是身体出问题了";
        let refined = "你帮我编辑一下信息，要给CTO说一下情况：目前我的情况是身体出问题了。";
        assert!(!looks_unsafe(raw, refined));
    }

    #[test]
    fn rejects_drops_and_inventions() {
        let raw = "我想让你帮我看看这是我们那个语音就是很多人不是去逆向Vesper Pro吗你想那个Temples那我们这种语音工具能不能也更新一下就是我们之前好像做过做的就是千问的好像放什么火山小引擎的那个什么APT";
        // 2026-11-03 qwen2.5:3b output: dropped half the content and invented a phrase.
        let dropped = "我想你们说的是很多人不是去逆向Vesper Pro吧？至于Temple的语音工具，是否可以更新得更精准呢？";
        assert!(looks_unsafe(raw, dropped));
        assert!(looks_unsafe("好的我知道了谢谢你啊", "好的我知道了，谢谢你。另外我还想补充一些完全不相关的新内容和要求"));
    }

    #[test]
    fn cleans_quotes_and_fences() {
        assert_eq!(clean_output("```\n“你好，世界。”\n```"), "你好，世界。");
    }

    #[test]
    fn ollama_url_stays_on_loopback() {
        std::env::set_var(OLLAMA_URL_ENV, "https://example.com/api/chat");
        assert_eq!(ollama_url(), DEFAULT_OLLAMA_URL);
        std::env::set_var(OLLAMA_URL_ENV, "http://127.0.0.1:11500/api/chat");
        assert_eq!(ollama_url(), "http://127.0.0.1:11500/api/chat");
        std::env::remove_var(OLLAMA_URL_ENV);
    }
}
