//! Read-only previews for Markdown files explicitly linked in a project message.
//! Clients receive opaque IDs, never an arbitrary-path file reader.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const TTL_SECONDS: u64 = 3600;

#[derive(Clone)]
struct Entry {
    root: PathBuf,
    path: PathBuf,
    touched: u64,
    image: bool,
}

#[derive(Default)]
pub(super) struct DocumentRegistry {
    entries: HashMap<String, Entry>,
}

pub(super) struct DocumentPreview {
    pub title: String,
    pub content: String,
}

fn decode_path(value: &str) -> Option<String> {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            output.push(u8::from_str_radix(pair, 16).ok()?);
            i += 3;
        } else if bytes[i] == b'\\' && i + 1 < bytes.len() {
            output.push(bytes[i + 1]);
            i += 2;
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(output).ok()?;
    (!decoded.chars().any(char::is_control)).then_some(decoded)
}

fn markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "md" | "markdown"))
        .unwrap_or(false)
}

fn image_content_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn supported_file(path: &Path, image: bool) -> bool {
    if image {
        image_content_type(path).is_some()
    } else {
        markdown_file(path)
    }
}

fn resolve_path(destination: &str, root: &Path, base: &Path, image: bool) -> Option<PathBuf> {
    let destination = destination.trim();
    let destination = if destination.starts_with('<') && destination.ends_with('>') {
        &destination[1..destination.len() - 1]
    } else {
        destination
    };
    let raw = destination.split('#').next()?;
    let mut decoded = decode_path(raw)?;
    if let Some(path) = decoded.strip_prefix("file://") {
        if !path.starts_with('/') {
            return None;
        }
        decoded = path.to_string();
    } else if decoded.contains("://") || decoded.starts_with("//") {
        return None;
    }
    // Editor suffixes such as README.md:12 or README.md:12:3.
    for _ in 0..2 {
        if let Some((path, line)) = decoded.rsplit_once(':') {
            if !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit()) {
                decoded = path.to_string();
            }
        }
    }
    let path = Path::new(&decoded);
    if !supported_file(path, image) {
        return None;
    }
    let canonical = if path.is_absolute() {
        path.canonicalize().ok()?
    } else {
        base.join(path).canonicalize().ok()?
    };
    if !canonical.starts_with(root) || !supported_file(&canonical, image) {
        return None;
    }
    let metadata = std::fs::metadata(&canonical).ok()?;
    (metadata.is_file()
        && metadata.len()
            <= if image {
                MAX_IMAGE_BYTES
            } else {
                MAX_DOCUMENT_BYTES
            })
    .then_some(canonical)
}

// Return link or image destinations, leaving escaped syntax and code untouched.
fn destination_ranges(text: &str, images: bool) -> Vec<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    let mut fence: Option<(u8, usize)> = None;
    while i < bytes.len() {
        if i == 0 || bytes[i - 1] == b'\n' {
            let mut start = i;
            while start < bytes.len() && start - i < 3 && bytes[start] == b' ' {
                start += 1;
            }
            let marker = *bytes.get(start).unwrap_or(&0);
            let count = bytes[start..]
                .iter()
                .take_while(|byte| **byte == marker)
                .count();
            if matches!(marker, b'`' | b'~') && count >= 3 {
                if fence
                    .map(|(old, n)| old == marker && count >= n)
                    .unwrap_or(false)
                {
                    fence = None;
                } else if fence.is_none() {
                    fence = Some((marker, count));
                }
                i = bytes[start..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|n| start + n + 1)
                    .unwrap_or(bytes.len());
                continue;
            }
        }
        if fence.is_some() {
            i += 1;
            continue;
        }
        if bytes[i] == b'\\' {
            i = (i + 2).min(bytes.len());
            continue;
        }
        if bytes[i] == b'`' {
            let count = bytes[i..].iter().take_while(|byte| **byte == b'`').count();
            let marker = "`".repeat(count);
            if let Some(end) = text[i + count..].find(&marker) {
                i += count + end + count;
            } else {
                i += count;
            }
            continue;
        }
        if bytes[i] != b'[' || (i > 0 && bytes[i - 1] == b'!') != images {
            i += 1;
            continue;
        }
        // An escaped exclamation mark is literal text, not an image.
        if images
            && bytes[..i - 1]
                .iter()
                .rev()
                .take_while(|byte| **byte == b'\\')
                .count()
                % 2
                == 1
        {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut brackets = 1;
        while j < bytes.len() && brackets > 0 && bytes[j] != b'\n' {
            match bytes[j] {
                b'\\' => j += 1,
                b'[' => brackets += 1,
                b']' => brackets -= 1,
                _ => {}
            }
            j += 1;
        }
        if brackets != 0 || bytes.get(j) != Some(&b'(') {
            i += 1;
            continue;
        }
        let start = j + 1;
        j = start;
        let mut depth = 1;
        let angle = bytes.get(start) == Some(&b'<');
        let mut in_angle = angle;
        while j < bytes.len() && depth > 0 && bytes[j] != b'\n' {
            match bytes[j] {
                b'\\' => j += 1,
                b'>' if in_angle => in_angle = false,
                b'(' if !in_angle => depth += 1,
                b')' if !in_angle => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        if depth == 0 {
            ranges.push(start..j - 1);
            i = j;
        } else {
            i += 1;
        }
    }
    ranges
}

impl DocumentRegistry {
    fn prune(&mut self, now: u64) {
        self.entries
            .retain(|_, entry| now.saturating_sub(entry.touched) < TTL_SECONDS);
        while self.entries.len() > MAX_ENTRIES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(id, _)| id.clone())
            {
                self.entries.remove(&oldest);
            }
        }
    }

    pub fn rewrite(
        &mut self,
        text: &str,
        root: &Path,
        base: &Path,
        now: u64,
        new_id: &mut impl FnMut() -> String,
    ) -> String {
        self.rewrite_resources(text, root, base, now, new_id, false)
    }

    fn rewrite_resources(
        &mut self,
        text: &str,
        root: &Path,
        base: &Path,
        now: u64,
        new_id: &mut impl FnMut() -> String,
        image: bool,
    ) -> String {
        let Ok(root) = root.canonicalize() else {
            return text.to_string();
        };
        if !root.is_dir() {
            return text.to_string();
        }
        self.prune(now);
        let mut output = String::with_capacity(text.len());
        let mut previous = 0;
        for range in destination_ranges(text, image) {
            let Some(path) = resolve_path(&text[range.clone()], &root, base, image) else {
                continue;
            };
            let existing = self
                .entries
                .iter()
                .find(|(_, entry)| entry.root == root && entry.path == path && entry.image == image)
                .map(|(id, _)| id.clone());
            let id = existing.unwrap_or_else(&mut *new_id);
            self.prune(now);
            self.entries.insert(
                id.clone(),
                Entry {
                    root: root.clone(),
                    path,
                    touched: now,
                    image,
                },
            );
            self.prune(now);
            output.push_str(&text[previous..range.start]);
            if image {
                output.push_str(&format!("/api/markdown-documents/{id}/image"));
            } else {
                output.push_str(&format!("iterate-document://{id}"));
            }
            previous = range.end;
        }
        output.push_str(&text[previous..]);
        output
    }

    pub fn read(
        &mut self,
        id: &str,
        now: u64,
        new_id: &mut impl FnMut() -> String,
    ) -> Result<DocumentPreview, &'static str> {
        self.prune(now);
        let entry = self
            .entries
            .get(id)
            .cloned()
            .ok_or("document_not_found_or_expired")?;
        if entry.image {
            return Err("document_not_found_or_expired");
        }
        let bytes = read_registered_file(&entry, MAX_DOCUMENT_BYTES)?;
        let canonical = &entry.path;
        let content = String::from_utf8(bytes).map_err(|_| "document_not_utf8")?;
        let title = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("文档")
            .to_string();
        let base = canonical.parent().ok_or("document_unavailable")?;
        let content = self.rewrite(&content, &entry.root, base, now, new_id);
        let content = self.rewrite_resources(&content, &entry.root, base, now, new_id, true);
        Ok(DocumentPreview { title, content })
    }

    pub fn read_image(
        &mut self,
        id: &str,
        now: u64,
    ) -> Result<(Vec<u8>, &'static str), &'static str> {
        self.prune(now);
        let entry = self
            .entries
            .get(id)
            .filter(|entry| entry.image)
            .ok_or("image_not_found_or_expired")?;
        let content_type = image_content_type(&entry.path).ok_or("unsupported_image")?;
        Ok((read_registered_file(entry, MAX_IMAGE_BYTES)?, content_type))
    }
}

fn read_registered_file(entry: &Entry, limit: u64) -> Result<Vec<u8>, &'static str> {
    let canonical = entry
        .path
        .canonicalize()
        .map_err(|_| "document_unavailable")?;
    if canonical != entry.path
        || !canonical.starts_with(&entry.root)
        || !supported_file(&canonical, entry.image)
    {
        return Err("document_path_changed");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A replaced FIFO must not block the registry; never follow a
        // newly substituted final symlink between canonicalize and open.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options
        .open(&canonical)
        .map_err(|_| "document_unavailable")?;
    let metadata = file.metadata().map_err(|_| "document_unavailable")?;
    // Recheck the opened descriptor against the current canonical path;
    // reject symlink/parent replacement between resolution and opening.
    if entry.root.canonicalize().ok().as_ref() != Some(&entry.root)
        || canonical.canonicalize().ok().as_ref() != Some(&canonical)
    {
        return Err("document_path_changed");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = std::fs::metadata(&canonical).map_err(|_| "document_unavailable")?;
        if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
            return Err("document_path_changed");
        }
    }
    if !metadata.is_file() || metadata.len() > limit {
        return Err("document_too_large_or_not_regular");
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "document_unavailable")?;
    if bytes.len() as u64 > limit {
        return Err("document_too_large_or_not_regular");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_documents_are_scoped_readable_and_follow_relative_links() {
        let root = std::env::temp_dir().join(format!("iterate-doc-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("README.md"), "# 首页\n[下一篇](docs/下一篇.md)").unwrap();
        std::fs::write(root.join("docs/下一篇.md"), "下一篇正文").unwrap();
        let mut registry = DocumentRegistry::default();
        let mut count = 0;
        let mut ids = || {
            count += 1;
            format!("doc_{count}")
        };
        let input = "[首页](README.md:12)\n`[代码](README.md)`\n```md\n[代码块](README.md)\n```\n![图片](README.md)";
        let output = registry.rewrite(input, &root, &root, 10, &mut ids);
        assert_eq!(output.matches("iterate-document://").count(), 1);
        assert!(output.contains("`[代码](README.md)`"));
        let first = registry.read("doc_1", 11, &mut ids).unwrap();
        assert_eq!(first.title, "README.md");
        assert!(first.content.contains("[下一篇](iterate-document://doc_2)"));
        assert_eq!(
            registry.read("doc_2", 12, &mut ids).unwrap().content,
            "下一篇正文"
        );
        assert!(registry.read("doc_1", 4000, &mut ids).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_images_resolve_relative_absolute_and_encoded_paths() {
        let root = std::env::temp_dir().join(format!("iterate-doc-images-{}", std::process::id()));
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let picture = root.join("docs/图 (1).png");
        std::fs::write(&picture, b"image bytes").unwrap();
        let content = format!("![relative](<图 (1).png>)\n![absolute](<{}>)\n![encoded](图%20%281%29.png)\n`![code](图%20%281%29.png)`\n```md\n![code](图%20%281%29.png)\n```\n![web](https://example.com/a.png)", picture.display());
        std::fs::write(root.join("docs/readme.md"), content).unwrap();
        let mut registry = DocumentRegistry::default();
        let mut count = 0;
        let mut ids = || {
            count += 1;
            format!("doc_{count}")
        };
        registry.rewrite("[read](docs/readme.md)", &root, &root, 1, &mut ids);
        let preview = registry.read("doc_1", 2, &mut ids).unwrap();
        assert_eq!(
            preview
                .content
                .matches("/api/markdown-documents/doc_2/image")
                .count(),
            3
        );
        assert!(preview.content.contains("`![code](图%20%281%29.png)`"));
        assert!(preview
            .content
            .contains("![web](https://example.com/a.png)"));
        assert_eq!(registry.read_image("doc_2", 3).unwrap().0, b"image bytes");
        assert!(registry.read_image("doc_1", 3).is_err());
        assert!(registry.read("doc_2", 3, &mut ids).is_err());
        assert!(registry.read_image("doc_2", 4000).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_paths_reject_outside_missing_and_unsupported_files() {
        let root =
            std::env::temp_dir().join(format!("iterate-image-boundary-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let outside = root.with_extension("png");
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::write(root.join("text.txt"), b"text").unwrap();
        let oversized = std::fs::File::create(root.join("large.png")).unwrap();
        oversized.set_len(MAX_IMAGE_BYTES + 1).unwrap();
        let canonical = root.canonicalize().unwrap();
        for source in [
            outside.to_string_lossy().as_ref(),
            "missing.png",
            "text.txt",
            "large.png",
        ] {
            assert!(resolve_path(source, &canonical, &canonical, true).is_none());
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("escape.png")).unwrap();
            assert!(resolve_path("escape.png", &canonical, &canonical, true).is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn code_and_escaped_images_are_not_registered() {
        let text = r"\![escaped](a.png)
`![code](a.png)`
~~~md
![fenced](a.png)
~~~
![real](a.png)";
        let ranges = destination_ranges(text, true);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&text[ranges[0].clone()], "a.png");
    }

    #[cfg(unix)]
    #[test]
    fn image_replacement_is_rejected_without_blocking() {
        let root =
            std::env::temp_dir().join(format!("iterate-image-replaced-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("image.png");
        std::fs::write(&path, b"original").unwrap();
        let mut registry = DocumentRegistry::default();
        registry.rewrite_resources(
            "![image](image.png)",
            &root,
            &root,
            1,
            &mut || "doc_image".into(),
            true,
        );
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
        assert!(registry.read_image("doc_image", 2).is_err());
        std::fs::remove_file(&path).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(registry.read_image("doc_image", 3).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn replaced_fifo_is_rejected_without_blocking() {
        let root = std::env::temp_dir().join(format!("iterate-doc-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("readme.md");
        std::fs::write(&path, "safe").unwrap();
        let mut registry = DocumentRegistry::default();
        registry.rewrite("[read](readme.md)", &root, &root, 1, &mut || {
            "doc_fifo".into()
        });
        std::fs::remove_file(&path).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(registry
            .read("doc_fifo", 2, &mut || "unused".into())
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_paths_and_external_links_are_not_registered() {
        let root = std::env::temp_dir().join(format!("iterate-doc-guard-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a (中文).md"), "safe").unwrap();
        std::fs::write(root.join("secret.txt"), "private").unwrap();
        let mut registry = DocumentRegistry::default();
        let output = registry.rewrite("[OK](<a (中文).md>) [web](https://example.com/a.md) [text](secret.txt) [missing](missing.md) [outside](/etc/passwd)", &root, &root, 1, &mut || "doc_safe".to_string());
        assert_eq!(output.matches("iterate-document://").count(), 1);
        assert!(registry.read("made-up", 1, &mut || String::new()).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(root.join("a (中文).md")).unwrap();
            std::os::unix::fs::symlink("/etc/passwd", root.join("a (中文).md")).unwrap();
            assert!(registry.read("doc_safe", 2, &mut || String::new()).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
