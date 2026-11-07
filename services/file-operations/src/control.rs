//! Local control documents are regular files, never streams. Reads use a pinned descriptor
//! and pread so evidence cannot reopen a replaced path or mutate a shared file offset.
use crate::model::{Error, Result};
use rustix::fs::{Mode, OFlags, open, openat};
use std::{
    fs::{File, Metadata},
    os::{
        fd::OwnedFd,
        unix::fs::{FileExt, MetadataExt},
    },
    path::{Component, Path},
};

pub const MARKER_LIMIT: u64 = 1024 * 1024;
const DOCUMENT_LIMIT: u64 = 256 * 1024 * 1024;
fn failure(path: &Path, code: &str, message: &str) -> Error {
    Error::new(503, code, message)
        .at(path.display().to_string())
        .during("control_read")
}
fn parent(path: &Path) -> Result<OwnedFd> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(failure(
            path,
            "unsafe_control_file",
            "控制文件必须为不含 .. 的绝对路径",
        ));
    }
    let mut fd = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for c in path.parent().unwrap().components() {
        if let Component::Normal(p) = c {
            fd = openat(
                &fd,
                p,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| {
                Error::from(e)
                    .at(path.display().to_string())
                    .during("control_read")
            })?;
        }
    }
    Ok(fd)
}
fn open_file(path: &Path, flags: OFlags) -> Result<File> {
    let parent = parent(path)?;
    let file: File = openat(
        &parent,
        path.file_name().unwrap(),
        flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o666),
    )
    .map_err(|e| {
        Error::from(e)
            .at(path.display().to_string())
            .during("control_read")
    })?
    .into();
    if !file.metadata()?.is_file() {
        return Err(failure(
            path,
            "unsafe_control_file",
            "控制文件不是普通文件，保留现场并停止读取",
        ));
    }
    Ok(file)
}
fn same(a: &Metadata, b: &Metadata) -> bool {
    a.is_file()
        && b.is_file()
        && a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
/// Check the directory entry without opening it (in particular, never open a replacement FIFO).
pub fn verify_path(file: &File, path: &Path) -> Result<()> {
    let parent = parent(path)?;
    let entry = rustix::fs::statat(
        &parent,
        path.file_name().unwrap(),
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|e| {
        Error::from(e)
            .at(path.display().to_string())
            .during("control_read")
    })?;
    let owned = file.metadata()?;
    if !owned.is_file()
        || entry.st_dev as u64 != owned.dev()
        || entry.st_ino as u64 != owned.ino()
        || rustix::fs::FileType::from_raw_mode(entry.st_mode) != rustix::fs::FileType::RegularFile
    {
        return Err(failure(
            path,
            "control_file_changed",
            "控制文件路径已被替换，保留现场并停止读取",
        ));
    }
    Ok(())
}
pub fn read_held_snapshot(file: &File, path: &Path, limit: u64) -> Result<(String, Metadata)> {
    verify_path(file, path)?;
    let before = file.metadata()?;
    let too_large = || {
        failure(
            path,
            "control_file_too_large",
            &format!(
                "控制文件超过读取上限 {limit} 字节；核对文件规模后调整 FILE_OPERATIONS_CONTROL_MAX_BYTES（标记文件上限固定）"
            ),
        )
    };
    if before.len() > limit {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 65536];
    loop {
        let remaining = limit.saturating_sub(bytes.len() as u64);
        let size = buffer.len().min(remaining.saturating_add(1) as usize);
        let n = file
            .read_at(&mut buffer[..size], bytes.len() as u64)
            .map_err(|e| {
                Error::from(e)
                    .at(path.display().to_string())
                    .during("control_read")
            })?;
        if n == 0 {
            break;
        }
        if n as u64 > remaining {
            return Err(too_large());
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let after = file.metadata()?;
    verify_path(file, path)?;
    if !same(&before, &after) || after.len() != bytes.len() as u64 {
        return Err(failure(
            path,
            "control_file_changed",
            "控制文件读取期间发生变化，请重新核对",
        ));
    }
    String::from_utf8(bytes)
        .map(|text| (text, after))
        .map_err(|_| failure(path, "invalid_control_file", "控制文件不是有效 UTF-8 文本"))
}
pub fn read_held(file: &File, path: &Path, limit: u64) -> Result<String> {
    read_held_snapshot(file, path, limit).map(|(text, _)| text)
}
pub fn read(path: &Path, limit: u64) -> Result<Option<String>> {
    match open_file(path, OFlags::RDONLY) {
        Ok(file) => read_held(&file, path, limit).map(Some),
        // A missing entry/parent is optional; a dangling symlink, FIFO or I/O failure is not.
        Err(e) if e.code == "source_changed" => Ok(None),
        Err(e) => Err(e),
    }
}
pub fn read_document(path: &Path) -> Result<Option<String>> {
    let limit = match std::env::var("FILE_OPERATIONS_CONTROL_MAX_BYTES") {
        Ok(raw) => raw
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0 && *n < usize::MAX as u64)
            .ok_or_else(|| {
                failure(
                    path,
                    "configuration",
                    "FILE_OPERATIONS_CONTROL_MAX_BYTES 必须为正整数字节上限",
                )
            })?,
        Err(std::env::VarError::NotPresent) => DOCUMENT_LIMIT,
        Err(_) => {
            return Err(failure(
                path,
                "configuration",
                "FILE_OPERATIONS_CONTROL_MAX_BYTES 无效",
            ));
        }
    };
    read(path, limit)
}
pub fn open_executor(path: &Path) -> Result<File> {
    let file = open_file(path, OFlags::RDWR | OFlags::CREATE)?;
    verify_path(&file, path)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, files::WriteGate};
    use std::{fs, os::unix::fs::symlink, time::Instant};
    fn fixture() -> Config {
        let root = fs::canonicalize("/tmp").unwrap().join(format!(
            "face-admin-person-fileops-control-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("base")).unwrap();
        fs::write(root.join(".fileops-fixture"), "person file operations").unwrap();
        Config {
            base: root.join("base"),
            media: root.join("media"),
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            fixture: Some(root),
        }
    }
    #[test]
    fn held_gate_rejects_fifo_symlink_and_regular_replacement_without_reopening() {
        for kind in ["fifo", "symlink", "regular"] {
            let config = fixture();
            let gate = WriteGate::acquire(&config, "fixture-id").unwrap();
            let proof = gate.evidence().unwrap();
            assert_eq!(proof, gate.evidence().unwrap()); // pread does not consume the shared fd offset
            let path = config.base.join(".review-write.lock");
            let original = config.base.join("original-marker");
            fs::rename(&path, &original).unwrap();
            match kind {
                "fifo" => assert!(
                    std::process::Command::new("mkfifo")
                        .arg(&path)
                        .status()
                        .unwrap()
                        .success()
                ),
                "symlink" => symlink(&original, &path).unwrap(),
                _ => fs::copy(&original, &path).map(|_| ()).unwrap(),
            }
            let start = Instant::now();
            assert_eq!(gate.evidence().unwrap_err().code, "control_file_changed");
            assert!(gate.release().is_err());
            assert!(start.elapsed().as_secs_f64() < 1.0);
            assert!(fs::symlink_metadata(&path).is_ok());
            assert!(
                fs::read_to_string(&original)
                    .unwrap()
                    .contains("fixture-id")
            );
        }
    }
    #[test]
    fn large_documents_and_exact_limits_are_distinct_from_small_markers() {
        let config = fixture();
        let path = config.base.join("large-legacy.json");
        let body = format!(
            "{{\"legacy\":\"{}\"}}",
            "x".repeat(2 * MARKER_LIMIT as usize)
        );
        fs::write(&path, &body).unwrap();
        assert_eq!(read_document(&path).unwrap().unwrap(), body);
        assert_eq!(read(&path, body.len() as u64).unwrap().unwrap(), body);
        assert_eq!(
            read(&path, body.len() as u64 - 1).unwrap_err().code,
            "control_file_too_large"
        );
        assert_eq!(
            read(&path, MARKER_LIMIT).unwrap_err().code,
            "control_file_too_large"
        );
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(DOCUMENT_LIMIT + 1)
            .unwrap();
        assert_eq!(
            read_document(&path).unwrap_err().code,
            "control_file_too_large"
        );
        let gate = WriteGate::acquire(&config, "large-marker").unwrap();
        File::options()
            .write(true)
            .open(config.base.join(".review-write.lock"))
            .unwrap()
            .set_len(MARKER_LIMIT + 1)
            .unwrap();
        assert_eq!(gate.evidence().unwrap_err().code, "control_file_too_large");
    }
    #[test]
    fn optional_absence_does_not_hide_a_dangling_link_or_fifo() {
        let config = fixture();
        let path = config.base.join("document.json");
        assert!(read_document(&path).unwrap().is_none());
        symlink(config.base.join("missing"), &path).unwrap();
        assert!(read_document(&path).is_err());
        fs::remove_file(&path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            read_document(&path).unwrap_err().code,
            "unsafe_control_file"
        );
    }
}
