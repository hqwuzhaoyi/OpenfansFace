//! Read-only filesystem inventory. Pinned directory descriptors never follow symlinks;
//! admission and descriptor lifetime belong to the bounded media-read lane in main.
use crate::{
    config::Config,
    files,
    model::{Error, Result},
    person_registration::SourceIndex,
};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, Metadata},
    os::unix::fs::MetadataExt,
    path::Path,
};

// Bound traversal and response memory without silently truncating a person's inventory.
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;
fn too_large() -> Error {
    Error::new(
        413,
        "inventory_limit",
        "目录超过单次读取范围，请缩小目录后重试",
    )
}
fn video(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| {
            [
                "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "ts", "mts", "m2ts",
            ]
            .contains(&s.to_ascii_lowercase().as_str())
        })
}
fn entries(parent: &File, path: &Path, budget: &mut usize) -> Result<Vec<(String, FileType)>> {
    let mut result = vec![];
    for entry in Dir::read_from(parent)? {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes.starts_with(b".") {
            continue;
        }
        *budget += 1;
        if *budget > MAX_ENTRIES {
            return Err(too_large());
        }
        let name = entry.file_name().to_str().map_err(|_| {
            Error::new(409, "invalid_filename", "目录包含无法表示的文件名")
                .at(path.display().to_string())
        })?;
        let stat = statat(parent, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW)?;
        result.push((name.to_owned(), FileType::from_raw_mode(stat.st_mode)));
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}
fn child_directory(parent: &File, name: &str) -> Result<File> {
    Ok(openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?
    .into())
}
fn root_directory(path: &Path) -> Result<Option<File>> {
    match files::directory(path) {
        Ok(fd) => Ok(Some(fd.into())),
        Err(e) if e.code == "source_changed" => Ok(None),
        Err(e) => Err(e.at(path.display().to_string()).during("inventory")),
    }
}
fn names(config: &Config) -> Result<Vec<String>> {
    let root = config.creators();
    let Some(dir) = root_directory(&root)? else {
        return Ok(vec![]);
    };
    Ok(entries(&dir, &root, &mut 0)?
        .into_iter()
        .filter_map(|(name, kind)| (kind == FileType::Directory).then_some(name))
        .collect())
}
#[derive(Default)]
struct Inventory {
    files: Vec<(String, Metadata)>,
    folders: BTreeMap<String, usize>,
    count: usize,
    bytes: u64,
    entries: usize,
}
fn walk(
    dir: &File,
    path: &Path,
    root: &Path,
    depth: usize,
    collect: bool,
    result: &mut Inventory,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(too_large());
    }
    for (name, kind) in entries(dir, path, &mut result.entries)? {
        let child = path.join(&name);
        if kind == FileType::Directory {
            let fd = child_directory(dir, &name)
                .map_err(|e| e.at(child.display().to_string()).during("inventory"))?;
            walk(&fd, &child, root, depth + 1, collect, result)?;
        } else if kind == FileType::RegularFile && video(&name) {
            let fd: File = openat(
                dir,
                name.as_str(),
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| {
                Error::from(e)
                    .at(child.display().to_string())
                    .during("inventory")
            })?
            .into();
            let stat = fd.metadata()?;
            if !stat.is_file() {
                return Err(
                    Error::conflict("读取期间文件类型已变化").at(child.display().to_string())
                );
            }
            result.count += 1;
            result.bytes = result.bytes.checked_add(stat.len()).ok_or_else(too_large)?;
            if collect {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .ok_or_else(|| Error::conflict("目录名称无效"))?;
                *result
                    .folders
                    .entry(if relative.is_empty() { "." } else { relative }.into())
                    .or_default() += 1;
                result.files.push((
                    child
                        .to_str()
                        .ok_or_else(|| Error::conflict("文件名无效"))?
                        .into(),
                    stat,
                ));
            }
        }
    }
    Ok(())
}
pub fn list(config: &Config, name: Option<&str>, stats: bool) -> Result<Value> {
    if stats {
        let mut values = BTreeMap::new();
        for name in names(config)? {
            let root = config.creators().join(&name);
            let value=(|| {
                let dir=root_directory(&root)?.ok_or_else(|| Error::conflict("读取期间人物目录已消失"))?;
                let mut inventory=Inventory::default();
                walk(&dir,&root,&root,0,false,&mut inventory)?;
                Ok::<_,Error>(json!({"video_count":inventory.count,"total_bytes":inventory.bytes}))
            })().unwrap_or_else(|e| json!({"video_count":null,"total_bytes":null,"stats_error":format!("{} · {}", e.error,e.path.as_deref().unwrap_or(root.to_str().unwrap_or("")))}));
            values.insert(name, value);
        }
        return Ok(json!({"stats":values,"updated_at":crate::video_health::timestamp(config)?}));
    }
    let Some(input) = name else {
        return Ok(json!({"names":names(config)?}));
    };
    let name = crate::person_registration::canonical(config, input)?;
    let (_, deleted) = crate::person_registration::document(config, "people-deleted.json", false)?;
    if deleted.as_object().unwrap().contains_key(&name) {
        return Err(Error::new(404, "deleted_person", "该人物已删除"));
    }
    let root = config.creators().join(&name);
    let dir = root_directory(&root)?;
    let mut inventory = Inventory::default();
    if let Some(dir) = &dir {
        walk(dir, &root, &root, 0, true, &mut inventory)?;
    }
    let paths = inventory
        .files
        .iter()
        .map(|(p, _)| p.clone())
        .collect::<Vec<_>>();
    let index = SourceIndex::load(config, &paths)?;
    let files=inventory.files.into_iter().map(|(path,stat)| {
        // Unregistered media uses its playable path until the first mutation persists its ID.
        let vid=index.ids_for_path(&path).into_iter().next().unwrap_or_else(||path.clone());
        let p=Path::new(&path);
        let folder=p.parent().unwrap().strip_prefix(&root).unwrap().to_str().unwrap();
        json!({"vid":vid,"path":path,"name":p.file_name().unwrap().to_str().unwrap(),"folder":if folder.is_empty(){"."}else{folder},"size":stat.len(),"size_gb":stat.len() as f64/1073741824.0,"modified_ms":stat.mtime() as f64*1000.0+stat.mtime_nsec() as f64/1000000.0,"version":files::fingerprint(&path,&stat),"thumb":null,"evidence":[],"state":"archived","organized":true})
    }).collect::<Vec<_>>();
    Ok(
        json!({"name":name,"root":root,"files":files,"folders":inventory.folders.into_iter().map(|(name,count)|json!({"name":name,"count":count})).collect::<Vec<_>>(),"directory_exists":dir.is_some()}),
    )
}
