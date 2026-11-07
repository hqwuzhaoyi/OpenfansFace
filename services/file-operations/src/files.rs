//! All media mutations use pinned directory descriptors and atomic no-replace link publication.
use crate::{
    config::Config,
    model::{Error, FileMove, MoveScope, Plan, Result, ScopeRoot, hash},
};
use rustix::fs::{AtFlags, Mode, OFlags, linkat, open, openat, unlinkat};
use serde_json::json;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{fd::OwnedFd, unix::fs::MetadataExt},
    path::{Component, Path},
};

/// A pending durable marker survives a crash or a failed outcome journal write.
/// Path observations cannot resolve this marker: remote RPC completion may be late.
fn mutation<T>(
    fixture: Option<&Path>,
    path: &str,
    step: &str,
    record: &mut impl FnMut(&str) -> Result<()>,
    effect: impl FnOnce() -> std::io::Result<T>,
) -> Result<T> {
    record("filesystem_mutation_intent")?;
    let result = (|| {
        crate::fault::mutation_error(fixture, step, false)?;
        let value = effect().map_err(Error::mutation)?;
        crate::fault::mutation_error(fixture, step, true)?;
        Ok(value)
    })()
    .map_err(|e: Error| e.at(path).during(step));
    if result.is_ok() || result.as_ref().is_err_and(|e| e.mutation_outcome.is_none()) {
        record("filesystem_mutation_resolved")?;
    }
    result
}

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
pub(crate) fn directory(path: &Path) -> Result<OwnedFd> {
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct MoveEvidence {
    #[serde(default)]
    pub source_parent: serde_json::Value,
    #[serde(default)]
    pub destination_parent: serde_json::Value,
    #[serde(default)]
    pub copy: Option<CopyProof>,
    #[serde(default)]
    pub restored: Option<CopyProof>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CopyProof {
    pub temp: String,
    pub identity: serde_json::Value,
    pub digest: Option<String>,
}
fn same_inode(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    a["dev"] == b["dev"] && a["ino"] == b["ino"]
}
fn root(config: &Config, scope: &ScopeRoot) -> Result<std::path::PathBuf> {
    match scope {
        ScopeRoot::Person(person) => {
            validate_name(person)?;
            Ok(config.creators().join(person))
        }
        ScopeRoot::Unorganized => Ok(config.media.join("pikpak")),
        ScopeRoot::Staging => Ok(config.media.join("pikpak/.face-cleanup-staging")),
    }
}
fn scoped(config: &Config, scope: &MoveScope, file: &FileMove) -> Result<()> {
    for (path, scope) in [(&file.src, &scope.source), (&file.dst, &scope.destination)] {
        let path = Path::new(path);
        let root = root(config, scope)?;
        if matches!(scope, ScopeRoot::Unorganized)
            && (path.starts_with(config.creators())
                || path.starts_with(config.media.join("pikpak/.face-cleanup-staging")))
        {
            return Err(
                Error::new(400, "invalid_request", "归档来源不能来自人物或暂存区")
                    .at(path.display().to_string()),
            );
        }
        if !path.is_absolute()
            || !path.starts_with(&root)
            || path == root
            || path
                .components()
                .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
        {
            return Err(Error::new(400, "invalid_request", "文件路径超出批准范围")
                .at(path.display().to_string()));
        }
    }
    if file.src == file.dst {
        return Err(Error::conflict("源文件与目标路径相同"));
    }
    Ok(())
}
pub fn parent_identity(path: &str) -> Result<serde_json::Value> {
    let parent: File = directory(
        Path::new(path)
            .parent()
            .ok_or_else(|| Error::conflict("文件父目录无效"))?,
    )?
    .into();
    Ok(identity(&parent.metadata()?))
}
pub fn approved_directory(config: &Config, scope: &MoveScope, path: &Path) -> Result<bool> {
    Ok(path.is_absolute()
        && path.starts_with(root(config, &scope.destination)?)
        && path
            .components()
            .all(|p| matches!(p, Component::RootDir | Component::Normal(_))))
}
pub fn check_move(config: &Config, scope: &MoveScope, file: &FileMove) -> Result<fs::Metadata> {
    scoped(config, scope, file)?;
    crate::permissions::require_move_permissions(Path::new(&file.src), Path::new(&file.dst))?;
    let parent: File = directory(Path::new(&file.src).parent().unwrap())?.into();
    let source: File = openat(
        &parent,
        Path::new(&file.src).file_name().unwrap(),
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?
    .into();
    let stat = source.metadata()?;
    if !stat.is_file() || fingerprint(&file.src, &stat) != file.version {
        return Err(Error::conflict("源文件已变化").at(&file.src));
    }
    match fs::symlink_metadata(&file.dst) {
        Ok(_) => {
            return Err(Error::new(409, "target_exists", "目标文件已存在，不能覆盖").at(&file.dst));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(Error::from(e).at(&file.dst)),
    }
    Ok(stat)
}
pub struct PinnedMove {
    parent: File,
    destination_parent: File,
    source: File,
    pub src: String,
    pub dst: String,
    pub evidence: MoveEvidence,
    fixture: Option<std::path::PathBuf>,
}
pub fn prepare_move(config: &Config, scope: &MoveScope, file: &FileMove) -> Result<PinnedMove> {
    check_move(config, scope, file)?;
    PinnedMove::open(config, file, MoveEvidence::default())
}
pub fn recover_move(
    config: &Config,
    scope: &MoveScope,
    file: &FileMove,
    evidence: MoveEvidence,
) -> Result<PinnedMove> {
    scoped(config, scope, file)?;
    let pinned = PinnedMove::open(config, file, evidence)?;
    pinned.position(&file.version)?;
    Ok(pinned)
}
impl PinnedMove {
    fn open(config: &Config, file: &FileMove, mut evidence: MoveEvidence) -> Result<Self> {
        let parent: File = directory(Path::new(&file.src).parent().unwrap())?.into();
        let destination_parent: File = directory(Path::new(&file.dst).parent().unwrap())?.into();
        for (actual, expected) in [
            (&parent, &mut evidence.source_parent),
            (&destination_parent, &mut evidence.destination_parent),
        ] {
            let stat = identity(&actual.metadata()?);
            if !expected.is_null() && !same_inode(&stat, expected) {
                return Err(Error::conflict("文件目录已被替换"));
            }
            *expected = stat;
        }
        let mut source = None;
        for (dir, path) in [(&parent, &file.src), (&destination_parent, &file.dst)] {
            match openat(
                dir,
                Path::new(path).file_name().unwrap(),
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => {
                    source = Some(File::from(fd));
                    break;
                }
                Err(rustix::io::Errno::NOENT) => (),
                Err(e) => return Err(Error::from(e).at(path)),
            }
        }
        Ok(Self {
            parent,
            destination_parent,
            source: source.ok_or_else(|| Error::conflict("源文件与目标文件均不存在"))?,
            src: file.src.clone(),
            dst: file.dst.clone(),
            evidence,
            fixture: config.fixture.clone(),
        })
    }
    pub fn new(config: &Config, person: &str, src: &str, dst: &str) -> Result<Self> {
        if Path::new(src).parent() != Path::new(dst).parent() {
            return Err(Error::new(400, "invalid_request", "改名必须位于同一目录"));
        }
        let file = FileMove {
            src: src.into(),
            dst: dst.into(),
            vids: vec![],
            size: 0,
            version: String::new(),
        };
        let scope = MoveScope {
            source: ScopeRoot::Person(person.into()),
            destination: ScopeRoot::Person(person.into()),
        };
        scoped(config, &scope, &file)?;
        crate::permissions::require_move_permissions(Path::new(src), Path::new(dst))?;
        let pinned = Self::open(config, &file, MoveEvidence::default())?;
        if !pinned.source.metadata()?.is_file() {
            return Err(Error::conflict("只允许操作普通视频文件"));
        }
        match rustix::fs::statat(
            &pinned.destination_parent,
            Path::new(dst).file_name().unwrap(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => {
                return Err(Error::new(409, "target_exists", "目标文件已存在，不能覆盖").at(dst));
            }
            Err(rustix::io::Errno::NOENT) => (),
            Err(e) => return Err(Error::from(e).at(dst)),
        }
        Ok(pinned)
    }
    pub fn recover(config: &Config, plan: &Plan) -> Result<Self> {
        if Path::new(&plan.file().src).parent() != Path::new(&plan.file().dst).parent() {
            return Err(Error::conflict("恢复路径超出原范围"));
        }
        recover_move(
            config,
            &MoveScope {
                source: ScopeRoot::Person(plan.person.clone()),
                destination: ScopeRoot::Person(plan.person.clone()),
            },
            plan.file(),
            MoveEvidence::default(),
        )
    }
    fn verify_parents(&self) -> Result<()> {
        for (path, pinned) in [
            (&self.src, &self.parent),
            (&self.dst, &self.destination_parent),
        ] {
            let current: File = directory(Path::new(path).parent().unwrap())?.into();
            if !same_inode(
                &identity(&current.metadata()?),
                &identity(&pinned.metadata()?),
            ) {
                return Err(Error::conflict("文件目录已被替换").at(path));
            }
        }
        Ok(())
    }
    fn proof_matches(file: &mut File, proof: &CopyProof) -> Result<bool> {
        if identity(&file.metadata()?) != proof.identity {
            return Ok(false);
        }
        let Some(expected) = &proof.digest else {
            return Ok(false);
        };
        Ok(&digest(file)? == expected)
    }
    pub fn position(&self, expected: &str) -> Result<(bool, bool)> {
        self.verify_parents()?;
        let mut present = [false, false];
        for (index, (dir, path, proof)) in [
            (&self.parent, &self.src, &self.evidence.restored),
            (&self.destination_parent, &self.dst, &self.evidence.copy),
        ]
        .into_iter()
        .enumerate()
        {
            match openat(
                dir,
                Path::new(path).file_name().unwrap(),
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => {
                    let mut file: File = fd.into();
                    if !file.metadata()?.is_file()
                        || !(fingerprint(&self.src, &file.metadata()?) == expected
                            || proof.as_ref().is_some_and(|p| {
                                Self::proof_matches(&mut file, p).unwrap_or(false)
                            }))
                    {
                        return Err(Error::conflict("文件身份发生外部变化，不能继续恢复").at(path));
                    }
                    present[index] = true;
                }
                Err(rustix::io::Errno::NOENT) => (),
                Err(e) => return Err(Error::from(e).at(path)),
            }
        }
        for (parent, proof) in [
            (&self.destination_parent, &self.evidence.copy),
            (&self.parent, &self.evidence.restored),
        ] {
            if let Some(proof) = proof {
                match openat(
                    parent,
                    proof.temp.as_str(),
                    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(fd) => {
                        let mut file: File = fd.into();
                        if proof.identity.is_null()
                            || !same_inode(&identity(&file.metadata()?), &proof.identity)
                            || (proof.digest.is_some() && !Self::proof_matches(&mut file, proof)?)
                        {
                            return Err(Error::conflict("临时复制文件缺少所有权证据或已变化"));
                        }
                    }
                    Err(rustix::io::Errno::NOENT) => (),
                    Err(e) => return Err(e.into()),
                }
            }
        }
        if !present[0] && !present[1] {
            return Err(Error::conflict("源文件与目标文件均不存在"));
        }
        Ok((present[0], present[1]))
    }
    pub fn stat(&self) -> Result<fs::Metadata> {
        Ok(self.source.metadata()?)
    }
    pub fn verify(&self, expected: &str) -> Result<()> {
        self.verify_parents()?;
        let current: File = openat(
            &self.parent,
            Path::new(&self.src).file_name().unwrap(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        if fingerprint(&self.src, &current.metadata()?) != expected {
            return Err(
                Error::new(409, "source_changed", "源文件已变化，请重新预览").at(&self.src),
            );
        }
        Ok(())
    }
    pub fn publish_recorded(
        &self,
        expected: &str,
        record: &mut impl FnMut(&str, &MoveEvidence) -> Result<()>,
    ) -> Result<()> {
        self.verify(expected)?;
        self.link_destination(record)?;
        self.position(expected)?;
        Ok(())
    }
    fn link_destination(
        &self,
        record: &mut impl FnMut(&str, &MoveEvidence) -> Result<()>,
    ) -> Result<()> {
        mutation(
            self.fixture.as_deref(),
            &self.dst,
            "destination_publish",
            &mut |phase| record(phase, &self.evidence),
            || {
                linkat(
                    &self.parent,
                    Path::new(&self.src).file_name().unwrap(),
                    &self.destination_parent,
                    Path::new(&self.dst).file_name().unwrap(),
                    AtFlags::empty(),
                )?;
                self.destination_parent.sync_all()
            },
        )
    }
    pub fn publish_recoverable(
        &mut self,
        config: &Config,
        expected: &str,
        record: &mut impl FnMut(&str, &MoveEvidence) -> Result<()>,
    ) -> Result<()> {
        if self.evidence.copy.is_some() || crate::fault::copy_required(config)? {
            return self.copy_publish(false, expected, record);
        }
        self.verify(expected)?;
        // Fallback is permitted only for a known no-effect hard-link rejection.
        // Keep the errno check inside the syscall, not inferred from generic error text.
        let mut fallback = false;
        mutation(
            self.fixture.as_deref(),
            &self.dst,
            "destination_publish",
            &mut |phase| record(phase, &self.evidence),
            || match linkat(
                &self.parent,
                Path::new(&self.src).file_name().unwrap(),
                &self.destination_parent,
                Path::new(&self.dst).file_name().unwrap(),
                AtFlags::empty(),
            ) {
                Ok(()) => self.destination_parent.sync_all(),
                Err(e)
                    if [
                        rustix::io::Errno::XDEV,
                        rustix::io::Errno::OPNOTSUPP,
                        rustix::io::Errno::PERM,
                    ]
                    .contains(&e) =>
                {
                    fallback = true;
                    Ok(())
                }
                Err(e) => Err(e.into()),
            },
        )?;
        if fallback {
            self.copy_publish(false, expected, record)
        } else {
            self.position(expected).map(|_| ())
        }
    }
    fn copy_publish(
        &mut self,
        reverse: bool,
        expected: &str,
        record: &mut impl FnMut(&str, &MoveEvidence) -> Result<()>,
    ) -> Result<()> {
        let (from, to, from_parent, to_parent) = if reverse {
            (&self.dst, &self.src, &self.destination_parent, &self.parent)
        } else {
            (&self.src, &self.dst, &self.parent, &self.destination_parent)
        };
        let mut proof = if reverse {
            self.evidence.restored.clone()
        } else {
            self.evidence.copy.clone()
        };
        if proof.is_none() {
            proof = Some(CopyProof {
                temp: format!(".face-move-{}.tmp", uuid::Uuid::new_v4()),
                identity: serde_json::Value::Null,
                digest: None,
            });
            if reverse {
                self.evidence.restored = proof.clone();
            } else {
                self.evidence.copy = proof.clone();
            }
            record("copy_create_intent", &self.evidence)?;
        }
        let mut proof = proof.unwrap();
        if proof.identity.is_null() {
            // A pre-existing name without inode proof is never adopted after a crash.
            let output = mutation(
                self.fixture.as_deref(),
                to,
                "copy_create",
                &mut |phase| record(phase, &self.evidence),
                || {
                    let output: File = openat(
                        to_parent,
                        proof.temp.as_str(),
                        OFlags::RDWR
                            | OFlags::CREATE
                            | OFlags::EXCL
                            | OFlags::NOFOLLOW
                            | OFlags::CLOEXEC,
                        Mode::from_raw_mode(0o666),
                    )?
                    .into();
                    output.sync_all()?;
                    to_parent.sync_all()?;
                    Ok(output)
                },
            )?;
            proof.identity = identity(&output.metadata()?);
            if reverse {
                self.evidence.restored = Some(proof.clone());
            } else {
                self.evidence.copy = Some(proof.clone());
            }
            record("copy_created", &self.evidence)?;
        }
        if proof.digest.is_none() {
            let mut output: File = openat(
                to_parent,
                proof.temp.as_str(),
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?
            .into();
            if !same_inode(&identity(&output.metadata()?), &proof.identity) {
                return Err(Error::conflict("临时复制文件被替换"));
            }

            let mut input: File = openat(
                from_parent,
                Path::new(from).file_name().unwrap(),
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?
            .into();
            if if reverse {
                !Self::proof_matches(
                    &mut input,
                    self.evidence
                        .copy
                        .as_ref()
                        .ok_or_else(|| Error::conflict("缺少目标复制证据"))?,
                )?
            } else {
                fingerprint(&self.src, &input.metadata()?) != expected
            } {
                return Err(Error::conflict("复制源不属于原操作"));
            }
            input.seek(SeekFrom::Start(0))?;
            let before = identity(&input.metadata()?);
            mutation(
                self.fixture.as_deref(),
                to,
                "copy_write",
                &mut |phase| record(phase, &self.evidence),
                || {
                    output.set_len(0)?;
                    std::io::copy(&mut input, &mut output)?;
                    output.set_times(
                        fs::FileTimes::new().set_modified(input.metadata()?.modified()?),
                    )?;
                    output.sync_all()
                },
            )?;
            let checksum = digest(&mut input)?;
            if before != identity(&input.metadata()?) || digest(&mut output)? != checksum {
                return Err(Error::conflict("复制期间源文件变化或校验失败"));
            }
            proof.identity = identity(&output.metadata()?);
            proof.digest = Some(checksum);
            if reverse {
                self.evidence.restored = Some(proof.clone())
            } else {
                self.evidence.copy = Some(proof.clone())
            };
            record("copy_verified", &self.evidence)?;
        }
        self.verify_parents()?;
        let mut output: File = openat(
            to_parent,
            proof.temp.as_str(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        if !Self::proof_matches(&mut output, &proof)? {
            return Err(Error::conflict("临时复制文件校验失败"));
        }
        self.position(expected)?;
        record("copy_publish_intent", &self.evidence)?;
        mutation(
            self.fixture.as_deref(),
            to,
            "copy_publish",
            &mut |phase| record(phase, &self.evidence),
            || {
                rustix::fs::renameat_with(
                    to_parent,
                    proof.temp.as_str(),
                    to_parent,
                    Path::new(to).file_name().unwrap(),
                    rustix::fs::RenameFlags::NOREPLACE,
                )?;
                to_parent.sync_all()
            },
        )?;
        record("copy_published", &self.evidence)
    }
    #[cfg(test)]
    pub fn remove_source(&self, expected: &str) -> Result<()> {
        self.remove_source_recorded(expected, &mut |_, _| Ok(()))
    }
    pub fn remove_source_recorded(
        &self,
        expected: &str,
        record: &mut impl FnMut(&str, &MoveEvidence) -> Result<()>,
    ) -> Result<()> {
        self.verify(expected)?;
        if !self.position(expected)?.1 {
            return Err(Error::conflict("目标尚未发布"));
        }
        mutation(
            self.fixture.as_deref(),
            &self.src,
            "source_remove",
            &mut |phase| record(phase, &self.evidence),
            || {
                unlinkat(
                    &self.parent,
                    Path::new(&self.src).file_name().unwrap(),
                    AtFlags::empty(),
                )?;
                self.parent.sync_all()
            },
        )
    }
    pub fn compensate_recoverable(
        &mut self,
        config: &Config,
        expected: &str,
        record: &mut impl FnMut(&str, &MoveEvidence) -> Result<()>,
    ) -> Result<()> {
        let (source, target) = self.position(expected)?;
        if !source {
            if self.evidence.copy.is_some() {
                self.copy_publish(true, expected, record)?;
            } else {
                mutation(
                    self.fixture.as_deref(),
                    &self.src,
                    "compensation_source_publish",
                    &mut |phase| record(phase, &self.evidence),
                    || {
                        linkat(
                            &self.destination_parent,
                            Path::new(&self.dst).file_name().unwrap(),
                            &self.parent,
                            Path::new(&self.src).file_name().unwrap(),
                            AtFlags::empty(),
                        )?;
                        self.parent.sync_all()
                    },
                )?;
            }
        }
        crate::fault::checkpoint(config, "compensation_source_published")?;
        self.position(expected)?;
        if target {
            crate::fault::checkpoint(config, "compensation_destination_remove_intent")?;
            mutation(
                self.fixture.as_deref(),
                &self.dst,
                "compensation_destination_remove",
                &mut |phase| record(phase, &self.evidence),
                || {
                    unlinkat(
                        &self.destination_parent,
                        Path::new(&self.dst).file_name().unwrap(),
                        AtFlags::empty(),
                    )?;
                    self.destination_parent.sync_all()
                },
            )?;
            crate::fault::checkpoint(config, "compensation_destination_removed")?;
        }
        for (parent, proof) in [
            (&self.destination_parent, &self.evidence.copy),
            (&self.parent, &self.evidence.restored),
        ] {
            if let Some(proof) = proof {
                match openat(
                    parent,
                    proof.temp.as_str(),
                    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(fd) => {
                        let file: File = fd.into();
                        if !same_inode(&identity(&file.metadata()?), &proof.identity) {
                            return Err(Error::conflict("临时复制文件被替换"));
                        }
                        mutation(
                            self.fixture.as_deref(),
                            &proof.temp,
                            "copy_cleanup",
                            &mut |phase| record(phase, &self.evidence),
                            || {
                                unlinkat(parent, proof.temp.as_str(), AtFlags::empty())?;
                                parent.sync_all()
                            },
                        )?;
                    }
                    Err(rustix::io::Errno::NOENT) => (),
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }
}
fn digest(file: &mut File) -> Result<String> {
    use sha2::{Digest, Sha256};
    file.seek(SeekFrom::Start(0))?;
    let mut state = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        state.update(&buffer[..count]);
    }
    Ok(format!("{:x}", state.finalize()))
}

pub fn read_optional(path: &Path) -> Result<Option<String>> {
    crate::control::read_document(path)
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
pub fn claim_owner_for(config: &Config, id: &str, action: &str) -> Result<()> {
    if config.parity_root.is_some() {
        return Ok(());
    }
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
    if read_optional(
        &config
            .base
            .join("staging-operations")
            .join(id)
            .join("operation.json"),
    )?
    .is_some()
    {
        return Err(Error::conflict("此编号已有旧暂存执行记录"));
    }
    let expected = json!({"id":id,"backend":"rust","action":action});
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(serde_json::to_string(&expected)?.as_bytes())?;
            file.sync_all()?;
            sync_parent(&path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let value: serde_json::Value = serde_json::from_str(
                &crate::control::read(&path, crate::control::MARKER_LIMIT)?
                    .ok_or_else(|| Error::conflict("操作路由标记已消失"))?,
            )?;
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
        config.require_writer()?;
        let path = config.base.join(".review-write.lock");
        let mut file = OpenOptions::new()
            .read(true)
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
                &json!({"backend":"rust","operation":id,"pid":std::process::id(),"authority":config.state_dir(),"token":uuid::Uuid::new_v4().to_string()}),
            )?
            .as_bytes(),
        )?;
        file.sync_all()?;
        sync_parent(&path)?;
        Ok(Self { file, path })
    }
    pub fn evidence(&self) -> Result<serde_json::Value> {
        let (raw, stat) = crate::control::read_held_snapshot(
            &self.file,
            &self.path,
            crate::control::MARKER_LIMIT,
        )?;
        Ok(
            json!({"identity":identity(&stat),"marker":serde_json::from_str::<serde_json::Value>(&raw)?}),
        )
    }
    /// Caller must hold the process-lifetime executor flock. PID/TTL never confer authority.
    pub fn recover(config: &Config, id: &str, evidence: &serde_json::Value) -> Result<Self> {
        config.require_writer()?;
        let path = config.base.join(".review-write.lock");
        let file: File = open(
            &path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        let gate = Self { file, path };
        let actual = gate.evidence()?;
        if actual["marker"]["backend"] != "rust"
            || actual["marker"]["operation"] != id
            || actual["marker"]["authority"] != json!(config.state_dir())
        {
            return Err(Error::conflict("共享写锁不属于此执行权威，不能接管"));
        }
        if *evidence != actual {
            return Err(Error::conflict("共享写锁已被替换，不能接管"));
        }
        Ok(gate)
    }
    pub fn release(self) -> Result<()> {
        crate::control::verify_path(&self.file, &self.path)?;
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirectoryProof {
    pub path: String,
    pub temp: String,
    pub identity: serde_json::Value,
    pub parent: serde_json::Value,
}
/// Publish an owned empty directory without replacing a late arrival. Persist ownership
/// before publication so restart can distinguish our directory from an external one.
pub fn create_directory(
    path: &str,
    previous: Option<DirectoryProof>,
    record: &mut impl FnMut(&str, &DirectoryProof) -> Result<()>,
) -> Result<DirectoryProof> {
    let path = Path::new(path);
    let parent: File = directory(
        path.parent()
            .ok_or_else(|| Error::conflict("目标目录无效"))?,
    )?
    .into();
    let mut proof = if let Some(proof) = previous {
        if !same_inode(&identity(&parent.metadata()?), &proof.parent) {
            return Err(Error::conflict("创建目录的父目录已变化"));
        }
        proof
    } else {
        let proof = DirectoryProof {
            path: path.display().to_string(),
            temp: format!(".face-dir-{}", uuid::Uuid::new_v4()),
            identity: serde_json::Value::Null,
            parent: identity(&parent.metadata()?),
        };
        record("directory_create_intent", &proof)?;
        proof
    };
    if proof.identity.is_null() {
        let owned = mutation(
            None,
            &proof.path,
            "directory_create",
            &mut |phase| record(phase, &proof),
            || {
                rustix::fs::mkdirat(&parent, proof.temp.as_str(), Mode::from_raw_mode(0o777))?;
                let owned: File = openat(
                    &parent,
                    proof.temp.as_str(),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?
                .into();
                owned.sync_all()?;
                parent.sync_all()?;
                Ok(owned)
            },
        )?;
        proof.identity = identity(&owned.metadata()?);
        record("directory_owned", &proof)?;
    }
    match openat(
        &parent,
        path.file_name().unwrap(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => {
            let actual: File = fd.into();
            if !same_inode(&identity(&actual.metadata()?), &proof.identity) {
                return Err(
                    Error::conflict("目标目录被外部创建或替换").at(path.display().to_string())
                );
            }
        }
        Err(rustix::io::Errno::NOENT) => {
            let owned: File = openat(
                &parent,
                proof.temp.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?
            .into();
            if !same_inode(&identity(&owned.metadata()?), &proof.identity) {
                return Err(Error::conflict("临时目录被替换"));
            }
            record("directory_publish_intent", &proof)?;
            mutation(
                None,
                &proof.path,
                "directory_publish",
                &mut |phase| record(phase, &proof),
                || {
                    rustix::fs::renameat_with(
                        &parent,
                        proof.temp.as_str(),
                        &parent,
                        path.file_name().unwrap(),
                        rustix::fs::RenameFlags::NOREPLACE,
                    )?;
                    parent.sync_all()
                },
            )?;
        }
        Err(e) => return Err(e.into()),
    }
    record("directory_published", &proof)?;
    Ok(proof)
}
pub fn remove_owned_directory(
    proof: &DirectoryProof,
    record: &mut impl FnMut(&str, &DirectoryProof) -> Result<()>,
) -> Result<()> {
    let path = Path::new(&proof.path);
    let parent: File = directory(path.parent().unwrap())?.into();
    if !same_inode(&identity(&parent.metadata()?), &proof.parent) {
        return Err(Error::conflict("清理目录的父目录已变化"));
    }
    for name in [path.file_name().unwrap(), std::ffi::OsStr::new(&proof.temp)] {
        match openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let owned: File = fd.into();
                if !same_inode(&identity(&owned.metadata()?), &proof.identity) {
                    return Err(Error::conflict("清理目录已被替换"));
                }
                // ENOTEMPTY is safe: preserve any external additions, never recursive-delete.
                mutation(
                    None,
                    &proof.path,
                    "directory_remove",
                    &mut |phase| record(phase, proof),
                    || match unlinkat(&parent, name, AtFlags::REMOVEDIR) {
                        Ok(()) => parent.sync_all(),
                        Err(rustix::io::Errno::NOTEMPTY) => Ok(()),
                        Err(e) => Err(e.into()),
                    },
                )?;
            }
            Err(rustix::io::Errno::NOENT) => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub fn inspect_owned_directory(proof: &DirectoryProof) -> Result<()> {
    let path = Path::new(&proof.path);
    let parent: File = directory(
        path.parent()
            .ok_or_else(|| Error::conflict("目录证明路径无效"))?,
    )?
    .into();
    if !same_inode(&identity(&parent.metadata()?), &proof.parent) {
        return Err(Error::conflict("创建目录的父目录已变化"));
    }
    for name in [path.file_name().unwrap(), std::ffi::OsStr::new(&proof.temp)] {
        match openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let file: File = fd.into();
                if proof.identity.is_null()
                    || !same_inode(&identity(&file.metadata()?), &proof.identity)
                {
                    return Err(Error::conflict("创建目录缺少所有权证据或已变化"));
                }
            }
            Err(rustix::io::Errno::NOENT) => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Remove an accepted source directory after all stores committed. Never removes contents,
/// follows symlinks, or treats a replaced inode as ours. Missing is accepted only by the
/// caller's durable post-commit cleanup intent.
pub fn remove_source_directory(
    config: &Config,
    proof: &crate::person_directory::Directory,
    record: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let path = Path::new(&proof.src);
    let parent: File = match directory(path.parent().unwrap()) {
        Ok(fd) => fd.into(),
        Err(e) if e.code == "source_changed" => return Ok(()),
        Err(e) => return Err(e),
    };
    if !same_inode(&identity(&parent.metadata()?), &proof.parent) {
        return Err(Error::conflict("来源父目录已被替换"));
    }
    let owned: File = match openat(
        &parent,
        path.file_name().unwrap(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd.into(),
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !same_inode(&identity(&owned.metadata()?), &proof.identity) {
        return Err(Error::conflict("来源目录已被替换").at(&proof.src));
    }
    mutation(
        config.fixture.as_deref(),
        &proof.src,
        "source_directory_remove",
        record,
        || {
            unlinkat(&parent, path.file_name().unwrap(), AtFlags::REMOVEDIR)?;
            parent.sync_all()
        },
    )
}

/// Verify a committed destination after source-directory cleanup removed its parent.
/// This retains the same inode/content evidence as PinnedMove::position, including EXDEV copies.
pub fn inspect_committed_destination(file: &FileMove, evidence: &MoveEvidence) -> Result<()> {
    let parent: File = directory(Path::new(&file.dst).parent().unwrap())?.into();
    if !same_inode(&identity(&parent.metadata()?), &evidence.destination_parent) {
        return Err(Error::conflict("目标父目录已被替换").at(&file.dst));
    }
    let mut opened: File = openat(
        &parent,
        Path::new(&file.dst).file_name().unwrap(),
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?
    .into();
    if !opened.metadata()?.is_file()
        || !(fingerprint(&file.src, &opened.metadata()?) == file.version
            || evidence
                .copy
                .as_ref()
                .is_some_and(|p| PinnedMove::proof_matches(&mut opened, p).unwrap_or(false)))
    {
        return Err(Error::conflict("已提交文件身份已变化").at(&file.dst));
    }
    Ok(())
}

/// Open every parent and the leaf without following symlinks. Also used for read-only
/// canonical duplicate evidence, so a symlink can never become an approved target.
fn regular_file(path: &str) -> Result<File> {
    let p = Path::new(path);
    let parent = directory(
        p.parent()
            .ok_or_else(|| Error::conflict("文件父目录无效"))?,
    )?;
    let file: File = openat(
        &parent,
        p.file_name().ok_or_else(|| Error::conflict("文件名无效"))?,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?
    .into();
    if !file.metadata()?.is_file() {
        return Err(Error::conflict("路径不是普通文件").at(path));
    }
    Ok(file)
}
pub fn regular_stat(path: &str) -> Result<fs::Metadata> {
    Ok(regular_file(path)?.metadata()?)
}
/// Bounded full byte comparison and SHA-256, with pinned descriptors and path
/// fingerprints checked on both sides before and after the complete read.
pub fn identical_files(
    config: &Config,
    source: &FileMove,
    target: &str,
    target_version: &str,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    let mut left = regular_file(&source.src)?;
    let mut right = regular_file(target)?;
    let before = [left.metadata()?, right.metadata()?];
    let check = |left: &File, right: &File| -> Result<()> {
        for (file, initial) in [(left, &before[0]), (right, &before[1])] {
            let now = file.metadata()?;
            if (now.ctime(), now.ctime_nsec()) != (initial.ctime(), initial.ctime_nsec()) {
                return Err(Error::conflict(
                    "完整内容校验期间文件状态已变化，请重新核对",
                ));
            }
        }
        if fingerprint(&source.src, &left.metadata()?) != source.version
            || fingerprint(target, &right.metadata()?) != target_version
            || fingerprint(&source.src, &regular_stat(&source.src)?) != source.version
            || fingerprint(target, &regular_stat(target)?) != target_version
        {
            return Err(Error::conflict(
                "完整内容校验前后文件状态已变化，请重新核对",
            ));
        }
        Ok(())
    };
    check(&left, &right)?;
    let mut remaining = source.size;
    let mut a = vec![0; 1024 * 1024];
    let mut b = vec![0; 1024 * 1024];
    let mut left_sha = Sha256::new();
    let mut right_sha = Sha256::new();
    while remaining > 0 {
        let n = remaining.min(a.len() as u64) as usize;
        left.read_exact(&mut a[..n])?;
        right.read_exact(&mut b[..n])?;
        if a[..n] != b[..n] {
            return Err(Error::new(
                409,
                "content_mismatch",
                "完整内容校验不一致，源文件未移动。请改选“保留两份”",
            ));
        }
        left_sha.update(&a[..n]);
        right_sha.update(&b[..n]);
        remaining -= n as u64;
        crate::fault::checkpoint(config, "duplicate_comparison_chunk")?;
    }
    if left_sha.finalize() != right_sha.finalize() {
        return Err(Error::conflict("完整内容摘要校验不一致，源文件未移动"));
    }
    check(&left, &right)?;
    Ok(())
}

/// Permanent disposal pins the parent and regular source. It has no inverse operation.
pub struct PinnedDelete {
    parent: File,
    source: Option<File>,
    path: String,
    parent_proof: serde_json::Value,
}
impl PinnedDelete {
    pub fn open(
        config: &Config,
        path: &str,
        parent_proof: Option<&serde_json::Value>,
    ) -> Result<Self> {
        let p = Path::new(path);
        let root = crate::staging::root(config);
        if !p.is_absolute()
            || !p.starts_with(&root)
            || p == root
            || p.components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(Error::conflict("永久删除路径超出暂存根目录").at(path));
        }
        let parent: File =
            directory(p.parent().ok_or_else(|| Error::conflict("缺少父目录"))?)?.into();
        let proof = identity(&parent.metadata()?);
        if parent_proof.is_some_and(|v| !same_inode(v, &proof)) {
            return Err(Error::conflict("暂存父目录被替换，不能核对删除结果").at(path));
        }
        let source = match openat(
            &parent,
            p.file_name().unwrap(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => Some(File::from(fd)),
            Err(rustix::io::Errno::NOENT) => None,
            Err(e) => return Err(Error::from(e).at(path)),
        };
        if let Some(file) = &source
            && !file.metadata()?.is_file()
        {
            return Err(Error::conflict("仅允许永久删除普通文件").at(path));
        }
        Ok(Self {
            parent,
            source,
            path: path.into(),
            parent_proof: proof,
        })
    }
    pub fn proof(&self) -> Result<serde_json::Value> {
        let stat = self
            .source
            .as_ref()
            .ok_or_else(|| Error::conflict("暂存文件已消失"))?
            .metadata()?;
        let parent = self.parent.metadata()?;
        Ok(
            json!({"parent":self.parent_proof,"version":fingerprint(&self.path,&stat),"mode":stat.mode(),"uid":stat.uid(),"gid":stat.gid(),"parent_mode":parent.mode(),"parent_uid":parent.uid(),"parent_gid":parent.gid()}),
        )
    }
    pub fn present(&self) -> bool {
        self.source.is_some()
    }
    pub fn verify(&self, expected: &serde_json::Value) -> Result<()> {
        let current = self.proof()?;
        // Directory size/mtime change when siblings are removed; only inode and permission bits matter.
        if current["version"] != expected["version"]
            || !same_inode(&current["parent"], &expected["parent"])
            || [
                "mode",
                "uid",
                "gid",
                "parent_mode",
                "parent_uid",
                "parent_gid",
            ]
            .iter()
            .any(|k| current[k] != expected[k])
        {
            return Err(Error::conflict("文件身份或权限已变化，请重新预览").at(&self.path));
        }
        crate::permissions::writable_directory(
            Path::new(&self.path).parent().unwrap(),
            "staging_delete",
        )?;
        let named = std::fs::symlink_metadata(&self.path)?;
        if !named.is_file()
            || fingerprint(&self.path, &named) != expected["version"].as_str().unwrap_or("")
            || !same_inode(&parent_identity(&self.path)?, &expected["parent"])
        {
            return Err(Error::conflict("执行前暂存路径已变化").at(&self.path));
        }
        Ok(())
    }
    pub fn unlink(
        &self,
        config: &Config,
        expected: &serde_json::Value,
        mut record: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        self.verify(expected)?;
        mutation(
            config.fixture.as_deref(),
            &self.path,
            "staging_unlink",
            &mut record,
            || {
                unlinkat(
                    &self.parent,
                    Path::new(&self.path).file_name().unwrap(),
                    AtFlags::empty(),
                )
                .map_err(std::io::Error::from)?;
                self.parent.sync_all()
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Config, FileMove, MoveScope) {
        let root = fs::canonicalize("/tmp").unwrap().join(format!(
            "face-admin-person-fileops-shared-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("base")).unwrap();
        fs::write(root.join(".fileops-fixture"), "person file operations").unwrap();
        let config = Config {
            base: root.join("base"),
            media: root.join("media"),
            ledger: root.join("ledger.db"),
            bind: "127.0.0.1:0".into(),
            token: None,
            parity_root: None,
            fixture: None,
        };
        let src = config.creators().join("甲/input.mp4");
        let dst = config.creators().join("乙/output.mp4");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::write(&src, b"owned fixture bytes\0\xff").unwrap();
        let stat = fs::metadata(&src).unwrap();
        let src = src.display().to_string();
        let file = FileMove {
            src: src.clone(),
            dst: dst.display().to_string(),
            vids: vec!["v1".into()],
            size: stat.len(),
            version: fingerprint(&src, &stat),
        };
        let scope = MoveScope {
            source: ScopeRoot::Person("甲".into()),
            destination: ScopeRoot::Person("乙".into()),
        };
        (config, file, scope)
    }
    #[test]
    fn cross_parent_hard_link_recovers_and_compensates() {
        let (config, file, scope) = fixture();
        let mut pinned = prepare_move(&config, &scope, &file).unwrap();
        let evidence = pinned.evidence.clone();
        pinned
            .publish_recoverable(&config, &file.version, &mut |_, _| Ok(()))
            .unwrap();
        pinned.remove_source(&file.version).unwrap();
        drop(pinned);
        let mut recovered = recover_move(&config, &scope, &file, evidence).unwrap();
        assert_eq!(recovered.position(&file.version).unwrap(), (false, true));
        recovered
            .compensate_recoverable(&config, &file.version, &mut |_, _| Ok(()))
            .unwrap();
        assert_eq!(fs::read(&file.src).unwrap(), b"owned fixture bytes\0\xff");
        assert!(!Path::new(&file.dst).exists());
    }
    #[test]
    fn copy_recovery_and_compensation_use_owned_inode_and_full_digest() {
        let (config, file, scope) = fixture();
        let mut pinned = prepare_move(&config, &scope, &file).unwrap();
        let mut evidence = MoveEvidence::default();
        let mut phases = vec![];
        pinned
            .copy_publish(false, &file.version, &mut |phase, proof| {
                phases.push(phase.to_owned());
                evidence = proof.clone();
                Ok(())
            })
            .unwrap();
        assert!(phases.iter().any(|p| p == "copy_verified"));
        assert!(evidence.copy.as_ref().unwrap().digest.is_some());
        pinned.remove_source(&file.version).unwrap();
        drop(pinned);
        let mut recovered = recover_move(&config, &scope, &file, evidence.clone()).unwrap();
        let interrupted =
            recovered.compensate_recoverable(&config, &file.version, &mut |phase, proof| {
                evidence = proof.clone();
                if phase == "copy_published" {
                    Err(Error::conflict("fixture stop after reverse publication"))
                } else {
                    Ok(())
                }
            });
        assert!(interrupted.is_err());
        drop(recovered);
        let mut recovered = recover_move(&config, &scope, &file, evidence).unwrap();
        recovered
            .compensate_recoverable(&config, &file.version, &mut |_, _| Ok(()))
            .unwrap();
        assert_eq!(fs::read(&file.src).unwrap(), b"owned fixture bytes\0\xff");
        assert!(!Path::new(&file.dst).exists());
    }
    #[test]
    fn copy_never_replaces_a_late_target() {
        let (config, file, scope) = fixture();
        let mut pinned = prepare_move(&config, &scope, &file).unwrap();
        let result = pinned.copy_publish(false, &file.version, &mut |phase, _| {
            if phase == "copy_verified" {
                fs::write(&file.dst, b"external").unwrap();
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&file.dst).unwrap(), b"external");
        assert_eq!(fs::read(&file.src).unwrap(), b"owned fixture bytes\0\xff");
    }
    #[test]
    fn resumed_copy_rejects_same_size_external_change() {
        let (config, file, scope) = fixture();
        let mut pinned = prepare_move(&config, &scope, &file).unwrap();
        let mut proof = MoveEvidence::default();
        pinned
            .copy_publish(false, &file.version, &mut |_, e| {
                proof = e.clone();
                Ok(())
            })
            .unwrap();
        pinned.remove_source(&file.version).unwrap();
        drop(pinned);
        fs::write(&file.dst, b"foreign fixture byt\0\xff").unwrap();
        assert!(recover_move(&config, &scope, &file, proof).is_err());
    }
    #[test]
    fn unknown_temp_inode_blocks_recovery_before_cleanup() {
        let (config, file, scope) = fixture();
        let pinned = prepare_move(&config, &scope, &file).unwrap();
        let mut proof = pinned.evidence.clone();
        proof.copy = Some(CopyProof {
            temp: ".pending-fixture.tmp".into(),
            identity: serde_json::Value::Null,
            digest: None,
        });
        fs::write(
            Path::new(&file.dst)
                .parent()
                .unwrap()
                .join(".pending-fixture.tmp"),
            b"foreign",
        )
        .unwrap();
        drop(pinned);
        assert!(recover_move(&config, &scope, &file, proof).is_err());
        assert!(Path::new(&file.src).exists());
    }
    #[test]
    fn directory_proof_recovers_publication_and_preserves_foreign_children() {
        let (config, _, _) = fixture();
        let path = config.creators().join("乙/nested").display().to_string();
        let mut proof = None;
        assert!(
            create_directory(&path, None, &mut |phase, value| {
                proof = Some(value.clone());
                if phase == "directory_owned" {
                    Err(Error::conflict("fixture interrupted"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let proof = create_directory(&path, proof, &mut |_, _| Ok(())).unwrap();
        inspect_owned_directory(&proof).unwrap();
        fs::write(Path::new(&path).join("external.txt"), b"external").unwrap();
        remove_owned_directory(&proof, &mut |_, _| Ok(())).unwrap();
        assert!(Path::new(&path).exists());
        fs::remove_file(Path::new(&path).join("external.txt")).unwrap();
        remove_owned_directory(&proof, &mut |_, _| Ok(())).unwrap();
        assert!(!Path::new(&path).exists());
    }
    #[test]
    fn two_parent_replacement_is_rejected() {
        let (config, file, scope) = fixture();
        let pinned = prepare_move(&config, &scope, &file).unwrap();
        let evidence = pinned.evidence.clone();
        drop(pinned);
        let parent = Path::new(&file.dst).parent().unwrap();
        fs::rename(parent, parent.with_extension("old")).unwrap();
        fs::create_dir(parent).unwrap();
        assert!(recover_move(&config, &scope, &file, evidence).is_err());
    }
}
