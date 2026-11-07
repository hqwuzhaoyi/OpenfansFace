//! Whole-directory inventory shared by identity operations. Cleanup occurs only after commit.
use crate::{
    config::Config,
    files,
    model::{Error, Plan, Result},
    permissions,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Directory {
    pub src: String,
    pub dst: String,
    pub identity: Value,
    pub parent: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Inventory {
    pub directories: Vec<Directory>,
    pub files: Vec<String>,
}
fn inode(value: Value) -> Value {
    json!({"dev":value["dev"],"ino":value["ino"]})
}
pub fn inventory(source: &Path, target: &Path) -> Result<Inventory> {
    let mut result = Inventory {
        directories: vec![],
        files: vec![],
    };
    match source.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e.into()),
        Ok(_) => (),
    }
    fn walk(src: &Path, dst: &Path, result: &mut Inventory) -> Result<()> {
        permissions::writable_directory(src, "source_directory")?;
        permissions::writable_directory(src.parent().unwrap(), "source_directory")?;
        let stat = fs::symlink_metadata(src)?;
        if !stat.is_dir() || stat.file_type().is_symlink() {
            return Err(
                Error::conflict("人物目录包含符号链接或特殊文件").at(src.display().to_string())
            );
        }
        result.directories.push(Directory {
            src: src.display().to_string(),
            dst: dst.display().to_string(),
            identity: inode(files::identity(&stat)),
            parent: inode(files::identity(&fs::metadata(src.parent().unwrap())?)),
        });
        let mut entries = fs::read_dir(src)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(&entry.path(), &dst.join(entry.file_name()), result)?
            } else if kind.is_file() {
                result.files.push(entry.path().display().to_string());
            } else {
                return Err(Error::conflict("人物目录包含符号链接或特殊文件")
                    .at(entry.path().display().to_string()));
            }
            if result.files.len() > 5000 || result.directories.len() > 5000 {
                return Err(Error::new(
                    413,
                    "scope_too_large",
                    "整个人物操作最多处理 5000 个文件和目录",
                ));
            }
        }
        Ok(())
    }
    walk(source, target, &mut result)?;
    result.files.sort();
    result.directories.sort_by(|a, b| a.src.cmp(&b.src));
    Ok(result)
}
pub fn intent(plan: &Plan) -> Result<Option<Inventory>> {
    plan.execution
        .as_ref()
        .and_then(|e| e.data.get("directory_inventory"))
        .map(|v| serde_json::from_value(v.clone()).map_err(Into::into))
        .transpose()
}
pub fn roots(config: &Config, plan: &Plan) -> (PathBuf, PathBuf) {
    (
        config.creators().join(&plan.person),
        if plan.action == "delete-person" {
            crate::staging::root(config)
                .join("person-deletions")
                .join(&plan.id)
                .join(&plan.person)
        } else {
            config.creators().join(&plan.target_person)
        },
    )
}
pub fn approved(config: &Config, plan: &Plan, path: &str) -> bool {
    if !["rename-person", "delete-person"].contains(&plan.action.as_str()) {
        return false;
    }
    let (_, target) = roots(config, plan);
    intent(plan).ok().flatten().is_some_and(|i| {
        i.directories.iter().any(|d| {
            (d.dst == path && Path::new(path).starts_with(&target))
                || (plan.action == "delete-person"
                    && Path::new(&d.dst).starts_with(path)
                    && Path::new(path).starts_with(crate::staging::root(config)))
        })
    })
}
