//! "Mac 听写版": re-recognize the phone's own recording on the Mac with Whisper.
//!
//! Apple's live recognition on the phone keeps showing text as the user speaks, but it
//! cannot recover from wrong words (e.g. "iterate" heard as "enter it"). After a dictation
//! ends, the phone uploads the m4a; the bridge forwards it to a resident local Whisper
//! service (`scripts/speech-whisper-server.py`, large-v3-turbo kept in memory) with a
//! Simplified-Chinese prompt plus product vocabulary, then runs the result through the
//! existing Ollama cleanup (`speech_refine`). The phone shows it as a suggestion; nothing
//! replaces the input unless the user confirms.

use base64::Engine as _;
use serde::Deserialize;
use std::time::Duration;

const DEFAULT_WHISPER_URL: &str = "http://127.0.0.1:18765/transcribe";
const WHISPER_URL_ENV: &str = "ITERATE_SPEECH_WHISPER_URL";
pub const PROVIDER: &str = "whisper";
pub const WHISPER_MODEL: &str = "whisper-large-v3-turbo";
/// Whole request body (JSON with base64 audio). Read only after auth succeeds.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Decoded audio. AAC at ~64 kbps: 5 MiB is over ten minutes of speech.
pub const MAX_AUDIO_BYTES: usize = 5 * 1024 * 1024;
const MIN_AUDIO_BYTES: usize = 512;
const WHISPER_TIMEOUT_SECS: u64 = 30;
const MAX_HINTS: usize = 24;
const MAX_HINT_CHARS: usize = 32;

/// Product names that Apple's live recognizer keeps mishearing. Always first in the prompt.
pub const PRODUCT_TERMS: &[&str] = &[
    "iterate", "Relearn", "Codex", "MCP", "call_zhi", "zhi", "cunzhi", "寸止", "Claude",
];

#[derive(Debug, Default, Deserialize)]
pub struct SpeechAudioTranscribeRequest {
    /// What Apple's live recognition put into the input box (the "original" on the card).
    #[serde(default)]
    pub transcript: String,
    #[serde(default)]
    pub audio_base64: String,
    #[serde(default)]
    pub audio_format: Option<String>,
    /// The phone's own speech hints (user words, shortcuts), appended after PRODUCT_TERMS.
    #[serde(default)]
    pub hints: Vec<String>,
    #[serde(default)]
    pub predefined_options: Vec<String>,
    #[serde(default)]
    pub mode: Option<String>,
    /// 新手机端：先拿 Whisper 结果立刻显示，整理（7B）由手机随后单独调用，不在这次请求里等。
    #[serde(default)]
    pub defer_refine: bool,
}

/// Whisper 原文已经够干净（有标点、没有口头禅），就不必再交给 7B 整理。
pub fn needs_refine(text: &str) -> bool {
    let chars = text.chars().count();
    if chars < 12 {
        return false;
    }
    const FILLERS: &[&str] = &["嗯", "呃", "额", "啊啊", "那个那个", "就是就是", "然后然后", "对对对", "这个这个"];
    if FILLERS.iter().any(|f| text.contains(f)) {
        return true;
    }
    let punct = text.chars().filter(|c| "，。？！、；：,.?!;:".contains(*c)).count();
    // 平均每 25 个字至少一个标点，才算 Whisper 已经断好句
    punct * 25 < chars
}

#[derive(Debug, PartialEq)]
pub enum RequestError {
    InvalidJson,
    InvalidAudio,
    AudioTooLarge,
    UnsupportedFormat,
}

impl RequestError {
    pub fn code(&self) -> &'static str {
        match self {
            RequestError::InvalidJson => "invalid_json",
            RequestError::InvalidAudio => "invalid_audio",
            RequestError::AudioTooLarge => "audio_too_large",
            RequestError::UnsupportedFormat => "unsupported_audio_format",
        }
    }
}

pub struct DecodedRequest {
    pub request: SpeechAudioTranscribeRequest,
    pub audio: Vec<u8>,
    pub format: &'static str,
}

pub fn decode_request(body: &[u8]) -> Result<DecodedRequest, RequestError> {
    let request: SpeechAudioTranscribeRequest =
        serde_json::from_slice(body).map_err(|_| RequestError::InvalidJson)?;
    let format = match request.audio_format.as_deref().unwrap_or("m4a").to_ascii_lowercase().as_str() {
        "m4a" => "m4a",
        "mp4" => "mp4",
        "aac" => "aac",
        "wav" => "wav",
        "caf" => "caf",
        _ => return Err(RequestError::UnsupportedFormat),
    };
    // base64 inflates by 4/3; reject before decoding anything that cannot fit.
    if request.audio_base64.len() / 4 * 3 > MAX_AUDIO_BYTES + 3 {
        return Err(RequestError::AudioTooLarge);
    }
    let audio = base64::engine::general_purpose::STANDARD
        .decode(request.audio_base64.trim())
        .map_err(|_| RequestError::InvalidAudio)?;
    if audio.len() > MAX_AUDIO_BYTES {
        return Err(RequestError::AudioTooLarge);
    }
    if audio.len() < MIN_AUDIO_BYTES {
        return Err(RequestError::InvalidAudio);
    }
    Ok(DecodedRequest { request, audio, format })
}

fn whisper_url() -> String {
    super::speech_refine::local_service_url(std::env::var(WHISPER_URL_ENV).ok(), DEFAULT_WHISPER_URL)
}

/// Product names first, then the phone's hints; dedupe case-insensitively, drop junk.
pub fn vocabulary(hints: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut terms = Vec::new();
    for term in PRODUCT_TERMS.iter().map(|t| t.to_string()).chain(hints.iter().cloned()) {
        let trimmed = term.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > MAX_HINT_CHARS
            || trimmed.chars().any(|ch| ch.is_control() || "、，。\"'".contains(ch))
        {
            continue;
        }
        if seen.insert(trimmed.to_lowercase()) {
            terms.push(trimmed.to_string());
        }
        if terms.len() >= MAX_HINTS {
            break;
        }
    }
    terms
}

/// Simplified Chinese wording in the prompt keeps large-v3-turbo from answering in
/// Traditional characters; the word list biases it toward the product names.
pub fn initial_prompt(vocabulary: &[String]) -> String {
    format!(
        "以下是简体中文普通话的语音输入，可能夹杂英文产品名和命令词：{}。",
        vocabulary.join("、")
    )
}

/// Whisper capitalizes ("Iterate", "ReLearn") and uses ASCII commas between Chinese.
/// Restore the canonical spelling of vocabulary terms and full-width punctuation.
pub fn normalize_whisper_text(text: &str, vocabulary: &[String]) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    let is_cjk = |ch: char| ('\u{4e00}'..='\u{9fff}').contains(&ch);
    let mut out = String::with_capacity(text.len());
    for (index, &ch) in chars.iter().enumerate() {
        let prev_cjk = index > 0 && is_cjk(chars[index - 1]);
        let next_cjk = chars.get(index + 1).is_some_and(|c| is_cjk(*c));
        let mapped = match ch {
            ',' if prev_cjk || next_cjk => '，',
            '?' if prev_cjk => '？',
            '!' if prev_cjk => '！',
            ':' if prev_cjk && next_cjk => '：',
            ';' if prev_cjk => '；',
            _ => ch,
        };
        out.push(mapped);
    }
    out = apply_spoken_aliases(&out);
    for term in vocabulary {
        if term.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') && term.len() >= 3 {
            out = replace_ascii_word_case_insensitive(&out, term);
        }
    }
    out
}

fn replace_ascii_word_case_insensitive(text: &str, canonical: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = canonical.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find(&needle) {
        let start = cursor + found;
        let end = start + needle.len();
        let boundary = |pos: Option<&u8>| pos.map_or(true, |b| !(b.is_ascii_alphanumeric() || *b == b'_'));
        let left_ok = start == 0 || boundary(bytes.get(start - 1));
        let right_ok = boundary(bytes.get(end));
        out.push_str(&text[cursor..start]);
        if left_ok && right_ok {
            out.push_str(canonical);
        } else {
            out.push_str(&text[start..end]);
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Spoken forms Whisper produces for terms that are not real words ("call zhi" -> "callz").
const SPOKEN_ALIASES: &[(&str, &str)] = &[
    ("call zhi", "call_zhi"),
    ("call_z", "call_zhi"),
    ("callzhi", "call_zhi"),
    ("callz", "call_zhi"),
    ("call z", "call_zhi"),
    // Whisper 偶尔把 iterate 拼成这些（11-04 实测出现 itrate）
    ("itrate", "iterate"),
    ("itarate", "iterate"),
    ("iterrate", "iterate"),
    ("iturate", "iterate"),
    ("itterate", "iterate"),
    ("itereate", "iterate"),
];

/// Map spoken aliases back to their canonical term (ASCII, case-insensitive, word-bounded).
pub fn apply_spoken_aliases(text: &str) -> String {
    let mut out = text.to_string();
    for (alias, canonical) in SPOKEN_ALIASES {
        out = replace_ascii_alias(&out, alias, canonical);
    }
    out
}

fn replace_ascii_alias(text: &str, alias: &str, canonical: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = alias.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let boundary = |pos: Option<&u8>| pos.map_or(true, |b| !(b.is_ascii_alphanumeric() || *b == b'_'));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find(&needle) {
        let start = cursor + found;
        let end = start + needle.len();
        out.push_str(&text[cursor..start]);
        if (start == 0 || boundary(bytes.get(start - 1))) && boundary(bytes.get(end)) {
            out.push_str(canonical);
        } else {
            out.push_str(&text[start..end]);
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// ASCII words (2+ chars) in the Whisper text that the cleanup step dropped or rewrote,
/// e.g. Ollama turning "Book" into "书". Non-empty means: do not trust the cleanup.
pub fn dropped_ascii_words(whisper_text: &str, refined_text: &str) -> Vec<String> {
    let refined_lower = refined_text.to_ascii_lowercase();
    let mut dropped = Vec::new();
    for word in whisper_text
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|w| w.len() >= 2 && w.chars().any(|c| c.is_ascii_alphabetic()))
    {
        let lower = word.to_ascii_lowercase();
        if !refined_lower.contains(&lower) && !dropped.contains(&lower) {
            dropped.push(lower);
        }
    }
    dropped
}

/// Whisper's well-known silence hallucinations and runaway outputs. When it fires, the
/// phone just keeps Apple's text and shows nothing.
pub fn looks_hallucinated(apple_text: &str, whisper_text: &str) -> bool {
    const PHRASES: &[&str] = &[
        "请不吝点赞", "点赞订阅", "字幕由", "字幕制作", "谢谢观看", "感谢观看", "Amara.org",
        "明镜与点点", "优优独播剧场",
    ];
    if PHRASES.iter().any(|phrase| whisper_text.contains(phrase)) {
        return true;
    }
    let count = |s: &str| s.chars().filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation()).count();
    let apple = count(apple_text);
    let whisper = count(whisper_text);
    if whisper == 0 {
        return true;
    }
    // English words are long in characters and short in Chinese, so the band is generous;
    // it only catches "said one sentence, got a paragraph" (or the reverse).
    apple >= 4 && (whisper * 10 > apple * 35 || whisper * 10 < apple * 3)
}

pub async fn call_whisper(audio: &[u8], format: &str, prompt: &str) -> Result<String, String> {
    let client = super::speech_refine::local_service_client(Duration::from_secs(WHISPER_TIMEOUT_SECS))
        .map_err(|error| format!("whisper_client_error: {}", error))?;
    let response = client
        .post(whisper_url())
        .json(&serde_json::json!({
            "audio_base64": base64::engine::general_purpose::STANDARD.encode(audio),
            "format": format,
            "language": "zh",
            "initial_prompt": prompt,
        }))
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() {
                "whisper_unreachable".to_string()
            } else if error.is_timeout() {
                "whisper_timeout".to_string()
            } else {
                format!("whisper_request_error: {}", error)
            }
        })?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("whisper_parse_error: {}", error))?;
    if !status.is_success() || body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let detail = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("whisper_status_{}: {}", status.as_u16(), detail));
    }
    Ok(body.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json).unwrap()
    }

    #[test]
    fn decode_rejects_bad_and_oversized_audio() {
        assert_eq!(decode_request(b"not json").err(), Some(RequestError::InvalidJson));
        assert_eq!(
            decode_request(&body(serde_json::json!({"audio_base64": "!!!", "transcript": "x"}))).err(),
            Some(RequestError::InvalidAudio)
        );
        let tiny = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert_eq!(
            decode_request(&body(serde_json::json!({"audio_base64": tiny}))).err(),
            Some(RequestError::InvalidAudio)
        );
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_AUDIO_BYTES + 1024]);
        assert_eq!(
            decode_request(&body(serde_json::json!({"audio_base64": big}))).err(),
            Some(RequestError::AudioTooLarge)
        );
        let ok = base64::engine::general_purpose::STANDARD.encode(vec![1u8; 4096]);
        assert_eq!(
            decode_request(&body(serde_json::json!({"audio_base64": ok.clone(), "audio_format": "exe"}))).err(),
            Some(RequestError::UnsupportedFormat)
        );
        let decoded = decode_request(&body(serde_json::json!({"audio_base64": ok, "transcript": "你好"}))).unwrap();
        assert_eq!(decoded.audio.len(), 4096);
        assert_eq!(decoded.format, "m4a");
    }

    #[test]
    fn vocabulary_puts_products_first_and_dedupes() {
        let hints = vec!["ITERATE".to_string(), "肌肉记忆".to_string(), " ".to_string(), "a、b".to_string()];
        let terms = vocabulary(&hints);
        assert_eq!(terms[0], "iterate");
        assert_eq!(terms.iter().filter(|t| t.eq_ignore_ascii_case("iterate")).count(), 1);
        assert!(terms.contains(&"肌肉记忆".to_string()));
        assert!(!terms.iter().any(|t| t.contains('、')));
        assert!(initial_prompt(&terms).starts_with("以下是简体中文"));
        let many: Vec<String> = (0..100).map(|i| format!("word{i}")).collect();
        assert_eq!(vocabulary(&many).len(), MAX_HINTS);
    }

    #[test]
    fn normalizes_case_and_punctuation() {
        let vocab = vocabulary(&[]);
        assert_eq!(
            normalize_whisper_text("帮我打开Iterate的设置,然后把ReLearn的笔记同步一下", &vocab),
            "帮我打开iterate的设置，然后把Relearn的笔记同步一下"
        );
        // Word boundaries: "iterated"/"Codexes" are not rewritten, standalone words are.
        assert_eq!(normalize_whisper_text("ITERATED codexes CODEX", &vocab), "ITERATED codexes Codex");
    }

    #[test]
    fn needs_refine_only_for_messy_text() {
        assert!(!needs_refine("我们测试一下这个Whisper、flow，或者说这个iterate模型。"));
        assert!(needs_refine("嗯我们测试一下这个模型然后呢那个效果怎么样呢我觉得还行吧"));
        assert!(needs_refine("我们测试一下这个模型然后呢那个效果怎么样呢我觉得还行吧就这样子吧"));
        assert!(!needs_refine("好的"));
        assert_eq!(apply_spoken_aliases("这个itrate模型"), "这个iterate模型");
    }

    #[test]
    fn maps_spoken_aliases() {
        let vocab = vocabulary(&[]);
        assert_eq!(normalize_whisper_text("你用callz告诉我结果", &vocab), "你用call_zhi告诉我结果");
        assert_eq!(normalize_whisper_text("用 Call Zhi 推送", &vocab), "用 call_zhi 推送");
        assert_eq!(normalize_whisper_text("callzone 不动", &vocab), "callzone 不动");
    }

    #[test]
    fn detects_dropped_ascii_words() {
        assert_eq!(dropped_ascii_words("让Codex看看这个Book", "让Codex看看这本书"), vec!["book".to_string()]);
        assert!(dropped_ascii_words("嗯换成Whisper然后用MCP", "换成Whisper，然后用MCP。").is_empty());
    }

    #[test]
    fn flags_hallucinations() {
        assert!(looks_hallucinated("", "请不吝点赞 订阅 转发"));
        assert!(looks_hallucinated("帮我看一下", ""));
        assert!(looks_hallucinated("帮我看一下", "帮我看一下这个问题然后再把所有的东西全部重新写一遍再发给我看看好不好呀"));
        assert!(!looks_hallucinated("我刚才说的是enter it不是", "我刚才说的是iterate，不是"));
    }

    /// End-to-end against a running `scripts/speech-whisper-server.py` (and Ollama):
    /// ITERATE_SPEECH_E2E_AUDIO=a.m4a[,b.m4a] cargo test --lib speech_audio_e2e -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn speech_audio_e2e() {
        let paths = std::env::var("ITERATE_SPEECH_E2E_AUDIO").expect("ITERATE_SPEECH_E2E_AUDIO");
        let apple = std::env::var("ITERATE_SPEECH_E2E_APPLE").unwrap_or_default();
        for path in paths.split(',') {
            let audio = std::fs::read(path).unwrap();
            let vocab = vocabulary(&[]);
            let started = std::time::Instant::now();
            let raw = call_whisper(&audio, "m4a", &initial_prompt(&vocab)).await.unwrap();
            let whisper_ms = started.elapsed().as_millis();
            let text = normalize_whisper_text(&raw, &vocab);
            let started = std::time::Instant::now();
            let refined = crate::bridge::speech_refine::refine(&crate::bridge::speech_refine::SpeechTextRefineRequest {
                transcript: text.clone(),
                ..Default::default()
            })
            .await;
            println!(
                "{path}\n  whisper({whisper_ms}ms): {raw}\n  normalized: {text}\n  hallucinated_vs_apple: {}\n  refine({}ms): {:?}",
                looks_hallucinated(if apple.is_empty() { &text } else { &apple }, &text),
                started.elapsed().as_millis(),
                refined
            );
        }
    }

    #[test]
    fn whisper_url_stays_on_loopback() {
        std::env::set_var(WHISPER_URL_ENV, "https://example.com/transcribe");
        assert_eq!(whisper_url(), DEFAULT_WHISPER_URL);
        std::env::set_var(WHISPER_URL_ENV, "http://127.0.0.1:19999/transcribe");
        assert_eq!(whisper_url(), "http://127.0.0.1:19999/transcribe");
        std::env::remove_var(WHISPER_URL_ENV);
    }
}
