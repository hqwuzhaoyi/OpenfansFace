//! All media mutations use pinned directory descriptors and atomic no-replace link publication.
use crate::{
    config::Config,
    model::{Error, Result, hash},
};
use rustix::fs::{Access, AtFlags, Mode, OFlags, accessat, linkat, open, openat, unlinkat};
use serde_json::json;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{fd::OwnedFd, unix::fs::MetadataExt},
    path::{Component, Path},
};

pub fn validate_name(name: &str) -> Result<()> {
    if name.trim() != name
        || name.is_empty()
        || name.len() > 240
        || name.starts_with('.')
        || name.ends_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err(Error::new(
            400,
            "invalid_request",
            "名称无效，不能包含路径分隔符、控制字符或隐藏文件前缀",
        ));
    }
    let stem = name.split('.').next().unwrap().to_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(Error::new(400, "invalid_request", "不能使用保留文件名"));
    }
    Ok(())
}
pub fn mtime_ns(stat: &fs::Metadata) -> String {
    (i128::from(stat.mtime()) * 1_000_000_000 + i128::from(stat.mtime_nsec())).to_string()
}
pub fn fingerprint(path: &str, stat: &fs::Metadata) -> String {
    hash(&json!([
        path,
        stat.dev().to_string(),
        stat.ino().to_string(),
        stat.len().to_string(),
        mtime_ns(stat)
    ]))
}
pub fn identity(stat: &fs::Metadata) -> serde_json::Value {
    json!({"dev":stat.dev().to_string(),"ino":stat.ino().to_string(),"size":stat.len().to_string(),"mtime_ns":mtime_ns(stat)})
}
fn directory(path: &Path) -> Result<OwnedFd> {
    // Walk from /, including configured roots: no intermediary symlink may redirect a mutation.
    let mut fd = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(part) => {
                fd = openat(
                    &fd,
                    part,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?
            }
            _ => {
                return Err(Error::new(
                    400,
                    "invalid_request",
                    "路径必须是绝对路径且不含 ..",
                ));
            }
        }
    }
    Ok(fd)
}
pub struct PinnedMove {
    parent: File,
    source: File,
    pub src: String,
    pub dst: String,
}
impl PinnedMove {
    pub fn new(config: &Config, person: &str, src: &str, dst: &str) -> Result<Self> {
        let src_path = Path::new(src);
        let dst_path = Path::new(dst);
        let root = config.creators().join(person);
        if !src_path.starts_with(&root)
            || src_path == root
            || src_path.parent() != dst_path.parent()
            || src_path
                .components()
                .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
        {
            return Err(
                Error::new(400, "invalid_request", "文件路径超出人物目录或包含无效路径").at(src),
            );
        }
        let parent: File = directory(src_path.parent().unwrap())
            .map_err(|e| e.at(src))?
            .into();
        accessat(
            &parent,
            ".",
            Access::WRITE_OK | Access::EXEC_OK,
            AtFlags::EACCESS,
        )
        .map_err(|e| Error::from(e).at(src))?;
        let source: File = openat(
            &parent,
            src_path.file_name().unwrap(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| Error::from(e).at(src))?
        .into();
        if !source.metadata()?.is_file() {
            return Err(Error::new(409, "source_changed", "只允许操作普通视频文件").at(src));
        }
        match rustix::fs::statat(
            &parent,
            dst_path.file_name().unwrap(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => {
                return Err(Error::new(409, "target_exists", "目标文件已存在，不能覆盖").at(dst));
            }
            Err(rustix::io::Errno::NOENT) => (),
            Err(e) => return Err(Error::from(e).at(dst)),
        }
        Ok(Self {
            parent,
            source,
            src: src.into(),
            dst: dst.into(),
        })
    }
    pub fn stat(&self) -> Result<fs::Metadata> {
        Ok(self.source.metadata()?)
    }
    pub fn verify(&self, expected: &str) -> Result<()> {
        let current: File = openat(
            &self.parent,
            Path::new(&self.src).file_name().unwrap(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        if fingerprint(&self.src, &current.metadata()?) != expected
            || fingerprint(&self.src, &self.source.metadata()?) != expected
        {
            return Err(
                Error::new(409, "source_changed", "源文件已变化，请重新预览").at(&self.src),
            );
        }
        let current_parent: File = directory(Path::new(&self.src).parent().unwrap())?.into();
        if (
            current_parent.metadata()?.dev(),
            current_parent.metadata()?.ino(),
        ) != (self.parent.metadata()?.dev(), self.parent.metadata()?.ino())
        {
            return Err(Error::new(409, "source_changed", "文件所在目录已变化").at(&self.src));
        }
        Ok(())
    }
    pub fn publish(&self, expected: &str) -> Result<()> {
        self.verify(expected)?;
        linkat(
            &self.parent,
            Path::new(&self.src).file_name().unwrap(),
            &self.parent,
            Path::new(&self.dst).file_name().unwrap(),
            AtFlags::empty(),
        )
        .map_err(|e| Error::from(e).at(&self.dst))?;
        self.parent.sync_all()?;
        self.verify_destination(expected)
    }
    fn verify_destination(&self, expected: &str) -> Result<()> {
        self.verify(expected)?;
        let target: File = openat(
            &self.parent,
            Path::new(&self.dst).file_name().unwrap(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        if fingerprint(&self.src, &target.metadata()?) != expected {
            return Err(
                Error::new(409, "source_changed", "新路径文件身份不符，需核对恢复").at(&self.dst),
            );
        }
        Ok(())
    }
    pub fn remove_source(&self, expected: &str) -> Result<()> {
        self.verify_destination(expected)?;
        unlinkat(
            &self.parent,
            Path::new(&self.src).file_name().unwrap(),
            AtFlags::empty(),
        )
        .map_err(|e| Error::from(e).at(&self.src))?;
        self.parent.sync_all()?;
        Ok(())
    }
}

pub fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::from(e).at(path.display().to_string())),
    }
}
pub fn sync_parent(path: &Path) -> Result<()> {
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    // Metadata documents may replace only the after-image checked under the shared write gate.
    fs::rename(&temp, path)?;
    sync_parent(path)
}
pub fn claim_owner(config: &Config, id: &str) -> Result<()> {
    let dir = config.base.join("file-operation-routing");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.json"));
    if config
        .base
        .join("person-operations")
        .join(id)
        .join("operation.json")
        .exists()
        && !path.exists()
    {
        return Err(Error::conflict("此编号已有旧后端执行记录"));
    }
    let expected = json!({"id":id,"backend":"rust","action":"rename-file"});
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(serde_json::to_string(&expected)?.as_bytes())?;
            file.sync_all()?;
            sync_parent(&path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
            if value != expected {
                return Err(Error::conflict("此操作已绑定另一执行后端或操作类型"));
            }
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

pub struct WriteGate {
    file: File,
    path: std::path::PathBuf,
}
impl WriteGate {
    pub fn acquire(config: &Config, id: &str) -> Result<Self> {
        let path = config.base.join(".review-write.lock");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::new(409, "writer_busy", "正在保存或归档，请稍后重试")
                } else {
                    e.into()
                }
            })?;
        // From this point, failed durability keeps the marker; never infer its ownership by PID.
        file.write_all(
            serde_json::to_string(
                &json!({"backend":"rust","operation":id,"pid":std::process::id()}),
            )?
            .as_bytes(),
        )?;
        file.sync_all()?;
        sync_parent(&path)?;
        Ok(Self { file, path })
    }
    pub fn release(self) -> Result<()> {
        let actual = fs::symlink_metadata(&self.path)?;
        let owned = self.file.metadata()?;
        if actual.dev() != owned.dev() || actual.ino() != owned.ino() {
            return Err(Error::new(
                503,
                "recovery_required",
                "共享写锁已被替换，停止写入",
            ));
        }
        fs::remove_file(&self.path)?;
        sync_parent(&self.path)?;
        Ok(())
    }
}
// No Drop unlock: abnormal unwinding and uncertain results preserve the marker intentionally.
