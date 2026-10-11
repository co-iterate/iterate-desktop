use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub(super) const MAX_FILE_COUNT: usize = 5;
pub(super) const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub(super) const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
const UPLOAD_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MobileFileUploadError {
    pub(super) code: &'static str,
    pub(super) message: String,
}

impl MobileFileUploadError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug)]
struct DecodedUpload {
    filename: String,
    bytes: Vec<u8>,
}

pub(super) fn prepare_mcp_action_file_uploads(
    payload: &mut Value,
    request_id: Option<&str>,
) -> Result<Vec<String>, MobileFileUploadError> {
    let uploads_root = dirs::home_dir()
        .map(|home| home.join(".cunzhi").join("uploads"))
        .unwrap_or_else(|| std::env::temp_dir().join("cunzhi-uploads"));
    prepare_mcp_action_file_uploads_at(payload, request_id, &uploads_root)
}

fn prepare_mcp_action_file_uploads_at(
    payload: &mut Value,
    request_id: Option<&str>,
    uploads_root: &Path,
) -> Result<Vec<String>, MobileFileUploadError> {
    let object = payload
        .as_object_mut()
        .ok_or_else(|| MobileFileUploadError::new("invalid_file_payload", "回复数据格式无效"))?;

    // A phone may provide bytes, never a Mac path. Remove caller-supplied paths
    // before doing any work so only paths created by this Bridge can escape.
    object.remove("file_paths");
    let files_value = object
        .remove("files")
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let files = files_value
        .as_array()
        .ok_or_else(|| MobileFileUploadError::new("invalid_file_payload", "文件附件格式无效"))?;

    if files.is_empty() {
        object.insert("file_paths".to_string(), Value::Array(Vec::new()));
        return Ok(Vec::new());
    }
    if files.len() > MAX_FILE_COUNT {
        return Err(MobileFileUploadError::new(
            "too_many_files",
            format!("一次最多上传 {MAX_FILE_COUNT} 个文件"),
        ));
    }

    let mut decoded = Vec::with_capacity(files.len());
    let mut total_bytes = 0usize;
    for item in files {
        let filename = sanitize_filename(
            item.get("filename")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?;
        let raw_data = item.get("data").and_then(Value::as_str).ok_or_else(|| {
            MobileFileUploadError::new("invalid_file_payload", format!("文件 {filename} 缺少内容"))
        })?;
        let encoded = raw_data
            .split_once(',')
            .filter(|(prefix, _)| prefix.starts_with("data:"))
            .map(|(_, data)| data)
            .unwrap_or(raw_data);

        // Reject oversized input before allocating decoded bytes. Base64 needs
        // at most four encoded bytes for every three decoded bytes, plus padding.
        let max_encoded_len = ((MAX_FILE_BYTES + 2) / 3) * 4 + 4;
        if encoded.len() > max_encoded_len {
            return Err(MobileFileUploadError::new(
                "file_too_large",
                format!("文件 {filename} 超过 10 MB"),
            ));
        }
        let bytes = BASE64.decode(encoded).map_err(|_| {
            MobileFileUploadError::new("invalid_file_base64", format!("文件 {filename} 内容损坏"))
        })?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(MobileFileUploadError::new(
                "file_too_large",
                format!("文件 {filename} 超过 10 MB"),
            ));
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| MobileFileUploadError::new("files_too_large", "文件总大小超过 20 MB"))?;
        if total_bytes > MAX_TOTAL_BYTES {
            return Err(MobileFileUploadError::new(
                "files_too_large",
                "文件总大小超过 20 MB",
            ));
        }
        decoded.push(DecodedUpload { filename, bytes });
    }

    create_private_directory(uploads_root)?;
    prune_expired_upload_directories(uploads_root);
    let scope = request_scope(request_id);
    let request_dir = uploads_root.join(scope);
    ensure_private_request_directory(&request_dir)?;

    let mut created_paths = Vec::new();
    let write_result = decoded
        .iter()
        .map(|upload| write_upload(&request_dir, upload, &mut created_paths))
        .collect::<Result<Vec<_>, _>>();

    let paths = match write_result {
        Ok(paths) => paths,
        Err(error) => {
            for path in created_paths {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
    };

    let path_strings = paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    object.insert("file_paths".to_string(), serde_json::json!(path_strings));
    Ok(path_strings)
}

fn sanitize_filename(raw: &str) -> Result<String, MobileFileUploadError> {
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let mut filename = leaf
        .chars()
        .filter(|character| *character != '\0')
        .map(|character| {
            if character.is_control() {
                '_'
            } else {
                character
            }
        })
        .take(180)
        .collect::<String>();
    filename = filename.trim().trim_matches('.').to_string();
    if filename.is_empty() {
        return Err(MobileFileUploadError::new("invalid_filename", "文件名无效"));
    }
    Ok(filename)
}

fn request_scope(request_id: Option<&str>) -> String {
    let normalized = request_id
        .unwrap_or_default()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .take(96)
        .collect::<String>();
    if normalized.trim_matches(['.', '_']).is_empty() {
        format!("request-{}", uuid::Uuid::new_v4())
    } else {
        format!("request-{normalized}")
    }
}

fn create_private_directory(path: &Path) -> Result<(), MobileFileUploadError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(MobileFileUploadError::new(
                "unsafe_upload_directory",
                "文件上传目录不安全",
            ));
        }
    }
    fs::create_dir_all(path).map_err(|error| {
        MobileFileUploadError::new(
            "upload_directory_failed",
            format!("无法创建文件上传目录: {error}"),
        )
    })?;
    set_mode(path, 0o700)
}

fn ensure_private_request_directory(path: &Path) -> Result<(), MobileFileUploadError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(MobileFileUploadError::new(
                "unsafe_upload_directory",
                "文件上传目录不安全",
            ));
        }
    }
    create_private_directory(path)
}

fn write_upload(
    request_dir: &Path,
    upload: &DecodedUpload,
    created_paths: &mut Vec<PathBuf>,
) -> Result<PathBuf, MobileFileUploadError> {
    let destination = unique_destination(request_dir, &upload.filename, &upload.bytes)?;
    if destination.exists() && existing_file_matches(&destination, &upload.bytes) {
        return Ok(destination);
    }

    let temporary = request_dir.join(format!(".upload-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|error| {
        MobileFileUploadError::new(
            "upload_write_failed",
            format!("无法保存文件 {}: {error}", upload.filename),
        )
    })?;
    if let Err(error) = file
        .write_all(&upload.bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&temporary, &destination))
    {
        let _ = fs::remove_file(&temporary);
        return Err(MobileFileUploadError::new(
            "upload_write_failed",
            format!("无法保存文件 {}: {error}", upload.filename),
        ));
    }
    set_mode(&destination, 0o600)?;
    created_paths.push(destination.clone());
    Ok(destination)
}

fn unique_destination(
    request_dir: &Path,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, MobileFileUploadError> {
    let original = request_dir.join(filename);
    if !original.exists() || existing_file_matches(&original, bytes) {
        return Ok(original);
    }

    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let extension = path.extension().and_then(|value| value.to_str());
    for suffix in 2..=999 {
        let candidate_name = match extension {
            Some(extension) if !extension.is_empty() => format!("{stem}-{suffix}.{extension}"),
            _ => format!("{stem}-{suffix}"),
        };
        let candidate = request_dir.join(candidate_name);
        if !candidate.exists() || existing_file_matches(&candidate, bytes) {
            return Ok(candidate);
        }
    }
    Err(MobileFileUploadError::new(
        "filename_conflict",
        format!("文件名冲突过多: {filename}"),
    ))
}

fn existing_file_matches(path: &Path, expected: &[u8]) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != expected.len() as u64
    {
        return false;
    }
    let mut bytes = Vec::with_capacity(expected.len());
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .is_ok()
        && bytes == expected
}

fn prune_expired_upload_directories(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let expired = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > UPLOAD_RETENTION);
        if expired {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), MobileFileUploadError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        MobileFileUploadError::new(
            "upload_permissions_failed",
            format!("无法保护文件上传目录: {error}"),
        )
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), MobileFileUploadError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_payload(filename: &str, bytes: &[u8]) -> Value {
        serde_json::json!({
            "action": "submit",
            "files": [{
                "filename": filename,
                "media_type": "application/octet-stream",
                "data": BASE64.encode(bytes),
            }]
        })
    }

    #[test]
    fn stages_a_file_under_the_private_request_directory() {
        let root = tempfile::tempdir().unwrap();
        let uploads = root.path().join("uploads");
        let mut payload = file_payload("notes.txt", b"hello");

        let paths = prepare_mcp_action_file_uploads_at(&mut payload, Some("request-123"), &uploads)
            .unwrap();

        assert_eq!(paths.len(), 1);
        assert_eq!(fs::read(&paths[0]).unwrap(), b"hello");
        assert!(paths[0].starts_with(uploads.to_string_lossy().as_ref()));
        assert!(payload.get("files").is_none());
        assert_eq!(payload["file_paths"][0], paths[0]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let directory_mode = fs::metadata(Path::new(&paths[0]).parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let file_mode = fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777;
            assert_eq!(directory_mode, 0o700);
            assert_eq!(file_mode, 0o600);
        }
    }

    #[test]
    fn strips_phone_paths_and_sanitizes_traversal_names() {
        let root = tempfile::tempdir().unwrap();
        let uploads = root.path().join("uploads");
        let mut payload = file_payload("../../private.txt", b"safe");
        payload["file_paths"] = serde_json::json!(["/etc/passwd"]);

        let paths =
            prepare_mcp_action_file_uploads_at(&mut payload, Some("abc"), &uploads).unwrap();

        assert_eq!(Path::new(&paths[0]).file_name().unwrap(), "private.txt");
        assert!(!payload.to_string().contains("/etc/passwd"));
    }

    #[test]
    fn rejects_too_many_or_oversized_files_without_partial_writes() {
        let root = tempfile::tempdir().unwrap();
        let uploads = root.path().join("uploads");
        let mut too_many = serde_json::json!({
            "files": (0..=MAX_FILE_COUNT).map(|index| serde_json::json!({
                "filename": format!("{index}.txt"),
                "data": BASE64.encode(b"x"),
            })).collect::<Vec<_>>()
        });
        assert_eq!(
            prepare_mcp_action_file_uploads_at(&mut too_many, Some("many"), &uploads)
                .unwrap_err()
                .code,
            "too_many_files"
        );

        let oversized = vec![0u8; MAX_FILE_BYTES + 1];
        let mut payload = file_payload("huge.bin", &oversized);
        assert_eq!(
            prepare_mcp_action_file_uploads_at(&mut payload, Some("huge"), &uploads)
                .unwrap_err()
                .code,
            "file_too_large"
        );
        assert!(!uploads.join("request-huge").exists());
    }

    #[test]
    fn retries_reuse_the_same_file_path() {
        let root = tempfile::tempdir().unwrap();
        let uploads = root.path().join("uploads");
        let mut first = file_payload("report.pdf", b"same bytes");
        let mut second = file_payload("report.pdf", b"same bytes");

        let first_path = prepare_mcp_action_file_uploads_at(&mut first, Some("retry"), &uploads)
            .unwrap()
            .remove(0);
        let second_path = prepare_mcp_action_file_uploads_at(&mut second, Some("retry"), &uploads)
            .unwrap()
            .remove(0);

        assert_eq!(first_path, second_path);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_upload_root() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let uploads = root.path().join("uploads");
        symlink(outside.path(), &uploads).unwrap();
        let mut payload = file_payload("notes.txt", b"do not escape");

        let error = prepare_mcp_action_file_uploads_at(&mut payload, Some("symlink"), &uploads)
            .unwrap_err();

        assert_eq!(error.code, "unsafe_upload_directory");
        assert!(!outside.path().join("request-symlink").exists());
    }
}
