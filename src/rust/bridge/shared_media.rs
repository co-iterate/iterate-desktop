//! Phone-openable links for video/audio files explicitly linked in a project message.
//!
//! The phone app hands ordinary https links to Safari, which carries no device
//! credentials, so a media link is rewritten to an unguessable, expiring
//! capability URL on the public bridge origin. Only files the message linked
//! inside its own project root are reachable; there is no path parameter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::markdown_documents::{decode_path, destination_ranges};

const MAX_MEDIA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
pub(super) const TTL_SECONDS: u64 = 12 * 3600;

#[derive(Clone)]
pub(super) struct SharedMedia {
    pub root: PathBuf,
    pub path: PathBuf,
    touched: u64,
}

#[derive(Default)]
pub(super) struct MediaRegistry {
    entries: HashMap<String, SharedMedia>,
}

pub(super) fn media_content_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "mp4" => Some("video/mp4"),
        "m4v" => Some("video/x-m4v"),
        "mov" => Some("video/quicktime"),
        "m4a" => Some("audio/mp4"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        _ => None,
    }
}

pub(super) fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn resolve_media_path(destination: &str, root: &Path) -> Option<PathBuf> {
    let destination = destination.trim();
    let destination = destination
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(destination);
    let mut decoded = decode_path(destination.split('#').next()?)?;
    if let Some(path) = decoded.strip_prefix("file://") {
        decoded = path.to_string();
    }
    if decoded.contains("://") || decoded.starts_with("//") {
        return None;
    }
    let path = Path::new(&decoded);
    media_content_type(path)?;
    let canonical = if path.is_absolute() {
        path.canonicalize().ok()?
    } else {
        root.join(path).canonicalize().ok()?
    };
    if !canonical.starts_with(root) || media_content_type(&canonical).is_none() {
        return None;
    }
    let metadata = std::fs::metadata(&canonical).ok()?;
    (metadata.is_file() && metadata.len() <= MAX_MEDIA_BYTES).then_some(canonical)
}

pub(super) fn encode_path_segment(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

impl MediaRegistry {
    fn prune(&mut self, now: u64) {
        self.entries
            .retain(|_, entry| now.saturating_sub(entry.touched) < TTL_SECONDS);
        while self.entries.len() > MAX_ENTRIES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// Rewrite `[text](/project/video.mp4)` to the share page
    /// `[text](https://<public>/api/shared-media/<token>)`.
    /// Without a public https origin the text is left unchanged.
    pub fn rewrite(
        &mut self,
        text: &str,
        root: &Path,
        public_base: &str,
        now: u64,
        new_token: &mut impl FnMut() -> String,
    ) -> String {
        let public_base = public_base.trim_end_matches('/');
        if !public_base.starts_with("https://") {
            return text.to_string();
        }
        let Ok(root) = root.canonicalize() else {
            return text.to_string();
        };
        if !root.is_dir() {
            return text.to_string();
        }
        self.prune(now);
        let mut output = String::with_capacity(text.len());
        let mut previous = 0;
        for range in destination_ranges(text, false) {
            let Some(path) = resolve_media_path(&text[range.clone()], &root) else {
                continue;
            };
            let existing = self
                .entries
                .iter()
                .find(|(_, entry)| entry.root == root && entry.path == path)
                .map(|(token, _)| token.clone());
            let token = existing.unwrap_or_else(&mut *new_token);
            self.entries.insert(
                token.clone(),
                SharedMedia {
                    root: root.clone(),
                    path: path.clone(),
                    touched: now,
                },
            );
            self.prune(now);
            output.push_str(&text[previous..range.start]);
            output.push_str(&format!("{public_base}/api/shared-media/{token}"));
            previous = range.end;
        }
        output.push_str(&text[previous..]);
        output
    }

    /// Look up a live token. Expiry counts from the last message that linked
    /// the file, not from playback, so a leaked URL cannot be kept alive.
    pub fn get(&mut self, token: &str, now: u64) -> Option<SharedMedia> {
        self.prune(now);
        self.entries.get(token).cloned()
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Safari can play a linked video but offers no "save to Photos" for it, so
/// the link opens this page: the player plus a button that hands the file to
/// the iOS share sheet ("存储视频", WeChat, ...) through the Web Share API.
pub(super) fn share_page_html(token: &str, path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("media");
    let content_type = media_content_type(path).unwrap_or("application/octet-stream");
    let src = format!("/api/shared-media/{token}/{}", encode_path_segment(name));
    let tag = if content_type.starts_with("audio/") {
        "audio"
    } else {
        "video"
    };
    let name_html = escape_html(name);
    // JSON is valid JS, but "</script>" inside a string would still end the
    // script element, so `<` is escaped as well.
    let js = |value: &str| {
        serde_json::to_string(value)
            .unwrap_or_else(|_| "\"\"".to_string())
            .replace('<', "\\u003c")
    };
    let (name_js, src_js, type_js) = (js(name), js(&src), js(content_type));
    format!(
        r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="referrer" content="no-referrer">
<title>{name_html}</title>
<style>
:root {{ color-scheme: light dark; font-family: -apple-system, system-ui, sans-serif; }}
body {{ margin: 0; padding: 16px; background: Canvas; color: CanvasText; }}
h1 {{ font-size: 15px; font-weight: 600; word-break: break-all; margin: 0 0 12px; }}
video, audio {{ width: 100%; border-radius: 10px; background: #000; }}
button, a.btn {{ display: block; width: 100%; box-sizing: border-box; margin-top: 14px; padding: 14px;
  border: 0; border-radius: 12px; font-size: 17px; font-weight: 600; text-align: center; text-decoration: none; }}
button {{ background: #0a84ff; color: #fff; }}
button:disabled {{ background: #8e8e93; }}
a.btn {{ background: rgba(127,127,127,.18); color: inherit; }}
p {{ font-size: 13px; opacity: .7; line-height: 1.5; }}
</style></head><body>
<h1>{name_html}</h1>
<{tag} src="{src}" controls playsinline preload="metadata"></{tag}>
<button id="share" disabled>正在准备文件…</button>
<a class="btn" href="{src}?download=1">下载到「文件」</a>
<p>点蓝色按钮，在弹出的菜单里选「存储视频」就会存进相册；也可以直接发给微信好友。链接 12 小时后失效。</p>
<script>
const name = {name_js}, src = {src_js}, type = {type_js};
const button = document.getElementById("share");
let file = null;
fetch(src).then(r => {{ if (!r.ok) throw new Error(r.status); return r.blob(); }})
  .then(blob => {{
    file = new File([blob], name, {{ type }});
    const ok = navigator.canShare && navigator.canShare({{ files: [file] }});
    button.disabled = false;
    button.textContent = ok ? "保存到相册 / 分享" : "下载";
  }})
  .catch(() => {{ button.textContent = "文件读取失败，请回 Mac 重新发送链接"; }});
button.addEventListener("click", async () => {{
  if (!file) return;
  if (navigator.canShare && navigator.canShare({{ files: [file] }})) {{
    try {{ await navigator.share({{ files: [file] }}); }} catch (e) {{}}
  }} else {{
    location.href = src + "?download=1";
  }}
}});
</script></body></html>"#
    )
}

/// Re-validate a registered file right before serving it.
pub(super) fn current_media_file(entry: &SharedMedia) -> Result<(PathBuf, u64), &'static str> {
    let canonical = entry.path.canonicalize().map_err(|_| "media_unavailable")?;
    if canonical != entry.path
        || entry.root.canonicalize().ok().as_ref() != Some(&entry.root)
        || !canonical.starts_with(&entry.root)
        || media_content_type(&canonical).is_none()
    {
        return Err("media_path_changed");
    }
    let metadata = std::fs::metadata(&canonical).map_err(|_| "media_unavailable")?;
    if !metadata.is_file() || metadata.len() > MAX_MEDIA_BYTES {
        return Err("media_too_large_or_not_regular");
    }
    Ok((canonical, metadata.len()))
}

/// Parse a single `Range: bytes=...` value against a file of `len` bytes.
/// `Ok(None)` means serve the whole file; `Err(())` means 416.
pub(super) fn parse_byte_range(value: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(value) = value.map(str::trim) else {
        return Ok(None);
    };
    let Some(spec) = value.strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        // Multipart ranges are not needed by media players; send the whole file.
        return Ok(None);
    }
    let (start, end) = spec.split_once('-').ok_or(())?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", "") => return Err(()),
        ("", suffix) => {
            let suffix: u64 = suffix.parse().map_err(|_| ())?;
            if suffix == 0 || len == 0 {
                return Err(());
            }
            (len.saturating_sub(suffix), len - 1)
        }
        (start, "") => (start.parse().map_err(|_| ())?, len.saturating_sub(1)),
        (start, end) => {
            let start: u64 = start.parse().map_err(|_| ())?;
            let end: u64 = end.parse().map_err(|_| ())?;
            (start, end.min(len.saturating_sub(1)))
        }
    };
    if len == 0 || start >= len || start > end {
        return Err(());
    }
    Ok(Some((start, end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("iterate-media-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("video-定义")).unwrap();
        root.canonicalize().unwrap()
    }

    fn token(n: u8) -> String {
        format!("{:064x}", n)
    }

    #[test]
    fn linked_project_media_becomes_public_capability_url() {
        let root = temp_root("rewrite");
        let video = root.join("video-定义/定义 成片.mp4");
        std::fs::write(&video, b"0123456789").unwrap();
        let mut registry = MediaRegistry::default();
        let mut n = 0;
        let text = format!(
            "看 [成片](<{}>) 和 `[不要](/x.mp4)`，[文档](README.md)",
            video.display()
        );
        let output = registry.rewrite(
            &text,
            &root,
            "https://iterate.example.com/",
            100,
            &mut || {
                n += 1;
                token(n)
            },
        );
        let url = format!("https://iterate.example.com/api/shared-media/{}", token(1));
        assert!(output.contains(&format!("[成片]({url})")), "{output}");
        assert!(output.contains("`[不要](/x.mp4)`"));
        assert!(output.contains("[文档](README.md)"));
        let entry = registry.get(&token(1), 200).unwrap();
        assert_eq!(current_media_file(&entry).unwrap(), (video.clone(), 10));
        let page = share_page_html(&token(1), &entry.path);
        assert!(page.contains(&format!(
            "src=\"/api/shared-media/{}/%E5%AE%9A%E4%B9%89%20%E6%88%90%E7%89%87.mp4\"",
            token(1)
        )));
        assert!(page.contains("<title>定义 成片.mp4</title>"));
        assert!(page.contains("navigator.share"));

        // File names cannot contain "/", but they can carry markup and quotes.
        let hostile = root.join("a<img src=x>\"'&.mp4");
        std::fs::write(&hostile, b"x").unwrap();
        let page = share_page_html(&token(2), &hostile);
        assert!(!page.contains("<img"), "{page}");
        assert!(page.contains("<title>a&lt;img src=x&gt;&quot;&#39;&amp;.mp4</title>"));
        assert!(page.contains(r#"const name = "a\u003cimg src=x>\"'&.mp4""#), "{page}");
        // The same file reuses its token; a second message refreshes expiry.
        let again = registry.rewrite(
            &text,
            &root,
            "https://iterate.example.com",
            100 + TTL_SECONDS - 1,
            &mut || token(9),
        );
        assert!(again.contains(&token(1)));
        assert!(registry.get(&token(1), 100 + 2 * TTL_SECONDS - 2).is_some());
        assert!(registry.get(&token(1), 100 + 3 * TTL_SECONDS).is_none());
    }

    #[test]
    fn outside_root_unsupported_or_without_public_origin_is_untouched() {
        let root = temp_root("scope");
        let outside =
            std::env::temp_dir().join(format!("iterate-media-outside-{}.mp4", std::process::id()));
        std::fs::write(&outside, b"x").unwrap();
        std::fs::write(root.join("notes.txt"), b"x").unwrap();
        std::fs::write(root.join("ok.mp4"), b"x").unwrap();
        let mut registry = MediaRegistry::default();
        let text = format!(
            "[a]({}) [b](notes.txt) [c](../{}) [d](https://x.com/v.mp4)",
            outside.display(),
            outside.file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(
            registry.rewrite(&text, &root, "https://p.example", 1, &mut || token(1)),
            text
        );
        // No https public origin: nothing is exposed.
        assert_eq!(
            registry.rewrite("[v](ok.mp4)", &root, "", 1, &mut || token(2)),
            "[v](ok.mp4)"
        );
        assert_eq!(
            registry.rewrite("[v](ok.mp4)", &root, "http://p.example", 1, &mut || token(
                3
            )),
            "[v](ok.mp4)"
        );
        assert!(registry.get(&token(2), 1).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn swapped_symlink_is_rejected_at_serve_time() {
        let root = temp_root("swap");
        let video = root.join("a.mp4");
        std::fs::write(&video, b"x").unwrap();
        let mut registry = MediaRegistry::default();
        registry.rewrite("[v](a.mp4)", &root, "https://p.example", 1, &mut || {
            token(4)
        });
        let entry = registry.get(&token(4), 1).unwrap();
        std::fs::remove_file(&video).unwrap();
        let secret =
            std::env::temp_dir().join(format!("iterate-media-secret-{}.mp4", std::process::id()));
        std::fs::write(&secret, b"secret").unwrap();
        std::os::unix::fs::symlink(&secret, &video).unwrap();
        assert_eq!(current_media_file(&entry), Err("media_path_changed"));
    }

    #[test]
    fn byte_ranges() {
        assert_eq!(parse_byte_range(None, 10), Ok(None));
        assert_eq!(parse_byte_range(Some("bytes=0-1"), 10), Ok(Some((0, 1))));
        assert_eq!(parse_byte_range(Some("bytes=4-"), 10), Ok(Some((4, 9))));
        assert_eq!(parse_byte_range(Some("bytes=-3"), 10), Ok(Some((7, 9))));
        assert_eq!(parse_byte_range(Some("bytes=5-100"), 10), Ok(Some((5, 9))));
        assert_eq!(parse_byte_range(Some("bytes=0-1,4-5"), 10), Ok(None));
        assert_eq!(parse_byte_range(Some("bytes=10-"), 10), Err(()));
        assert_eq!(parse_byte_range(Some("bytes=5-2"), 10), Err(()));
        assert_eq!(parse_byte_range(Some("bytes=-0"), 10), Err(()));
        assert_eq!(parse_byte_range(Some("bytes=x-1"), 10), Err(()));
        assert!(valid_token(&token(1)));
        assert!(!valid_token("abc"));
        assert!(!valid_token(&"g".repeat(64)));
    }
}
