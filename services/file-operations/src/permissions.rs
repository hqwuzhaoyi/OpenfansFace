//! Read-only server-side access checks. Never infer NFS permissions from Unix mode bits or UID 0.
use crate::model::{Error, Result};
use rustix::fs::{Access, AtFlags, Mode, OFlags, accessat, open, openat};
use std::{
    os::fd::OwnedFd,
    path::{Component, Path, PathBuf},
};

fn failure(error: rustix::io::Errno, path: &Path, step: &str) -> Error {
    let mut result = Error::from(error)
        .at(path.display().to_string())
        .during(step);
    result.error = format!("{step}：{}；{}", path.display(), result.error);
    result
}

/// Checks the nearest existing directory if descendants have yet to be created.
/// Walks every component without following symlinks; missing is distinct from I/O failure.
pub fn writable_directory(path: &Path, step: &str) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(Error::new(400, "invalid_request", "目录必须为绝对路径").during(step));
    }
    let mut current = PathBuf::from("/");
    let mut fd: OwnedFd = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| failure(e, &current, step))?;
    let parts: Vec<_> = path.components().collect();
    if parts
        .iter()
        .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
    {
        return Err(Error::new(400, "invalid_request", "目录不能包含 ..")
            .at(path.display().to_string())
            .during(step));
    }
    for part in parts {
        if let Component::Normal(part) = part {
            let next = current.join(part);
            match openat(
                &fd,
                part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(next_fd) => {
                    fd = next_fd;
                    current = next;
                }
                Err(rustix::io::Errno::NOENT) => break,
                Err(e) => return Err(failure(e, &next, step)),
            }
        }
    }
    accessat(
        &fd,
        ".",
        Access::WRITE_OK | Access::EXEC_OK,
        AtFlags::EACCESS,
    )
    .map_err(|e| failure(e, &current, step))?;
    Ok(current)
}

pub fn require_move_permissions(src: &Path, dst: &Path) -> Result<()> {
    let mut cache = std::collections::BTreeMap::new();
    require_move_permissions_cached(src, dst, &mut cache)
}

pub fn require_move_permissions_cached(
    src: &Path,
    dst: &Path,
    cache: &mut std::collections::BTreeMap<(PathBuf, &'static str), Result<PathBuf>>,
) -> Result<()> {
    for (dir, step) in [
        (
            src.parent()
                .ok_or_else(|| Error::new(400, "invalid_request", "源目录无效"))?,
            "source_directory",
        ),
        (
            dst.parent()
                .ok_or_else(|| Error::new(400, "invalid_request", "目标目录无效"))?,
            "destination_directory",
        ),
    ] {
        let key = (dir.to_path_buf(), step);
        let result = cache
            .entry(key)
            .or_insert_with(|| writable_directory(dir, step))
            .clone();
        result?;
    }
    // Do not wait for a writer if an external actor replaces a regular file with a FIFO.
    let source = open(
        src,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| failure(e, src, "source_read"))?;
    drop(source);
    Ok(())
}
