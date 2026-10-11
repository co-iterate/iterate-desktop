use std::io::Write;
use std::path::Path;

/// 先写同目录临时文件再 rename，读者要么看不到文件，要么看到完整内容。
///
/// `--serve` 轮询响应文件，一旦看到文件存在就会结束 GUI 子进程；直接 `fs::write`
/// 时可能读到空文件或半截 JSON，最后报 `No response from GUI`（P-2026-9339）。
pub fn write_file_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_file_atomically;

    #[test]
    fn writes_full_content_and_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iterate_response_test.json");
        write_file_atomically(&path, br#"{"user_input":"a"}"#).unwrap();
        write_file_atomically(&path, br#"{"user_input":"b"}"#).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"user_input":"b"}"#);
        let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "temporary files must not be left behind");
    }
}
