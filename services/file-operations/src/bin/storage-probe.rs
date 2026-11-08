//! Explicitly invoked acceptance utility, never an HTTP endpoint or production startup action.
#[allow(dead_code)]
#[path = "../model.rs"]
mod model;
#[path = "../permissions.rs"]
mod permissions;
use model::{Error, Result};
use serde_json::json;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

fn marked(root: &Path) -> Result<()> {
    if fs::canonicalize(root)? != root
        || !root
            .components()
            .any(|p| p.as_os_str() == ".face-admin-rust-fixtures")
        || fs::read_to_string(root.join(".rust-nfs-fixture"))? != "rust nfs acceptance\n"
    {
        return Err(Error::new(
            400,
            "unsafe_fixture",
            "需要独立标记的 .face-admin-rust-fixtures 子目录",
        ));
    }
    Ok(())
}
fn exercise(root: &Path) -> Result<serde_json::Value> {
    marked(root)?;
    let dir = root.join(format!("probe-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&dir)?; // Intentionally inherit the export's default ACL; never chmod.
    let src = dir.join("source.bin");
    let dst = dir.join("linked.bin");
    let copy = dir.join("exclusive-copy.bin");
    let data = b"isolated native Rust NFS acceptance\0\xff";
    let mut source = OpenOptions::new().write(true).create_new(true).open(&src)?;
    source.write_all(data)?;
    source.sync_all()?;
    permissions::require_move_permissions(&src, &dst)?;
    fs::hard_link(&src, &dst)?;
    assert_eq!(fs::metadata(&src)?.ino(), fs::metadata(&dst)?.ino());
    fs::remove_file(&src)?;
    // Guarded rollback: target still owns the original inode; source publication is no-replace.
    fs::hard_link(&dst, &src)?;
    fs::remove_file(&dst)?;
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&copy)?;
    std::io::copy(&mut File::open(&src)?, &mut target)?;
    target.sync_all()?;
    let occupied = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&copy)
        .unwrap_err();
    assert_eq!(occupied.kind(), std::io::ErrorKind::AlreadyExists);
    let mut bytes = Vec::new();
    File::open(&copy)?.read_to_end(&mut bytes)?;
    assert_eq!(bytes, data);
    let stat = fs::metadata(&src)?;
    let result = json!({"ok":true,"directory":dir,"effective_uid":rustix::process::geteuid().as_raw(),"effective_gid":rustix::process::getegid().as_raw(),"server_uid":stat.uid(),"server_gid":stat.gid(),"checks":["mkdir_inheritance","hard_link","source_unlink","no_replace_rollback","exclusive_copy","occupied_copy_preserved","fsync"]});
    // Retain the owned directory and files as UID/ACL evidence. The harness cleans only this run.
    File::open(&dir)?.sync_all()?;
    Ok(result)
}
fn run() -> Result<serde_json::Value> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("exercise") if args.len() == 3 => exercise(&PathBuf::from(&args[2])),
        Some("check") if args.len() == 5 => {
            let root = Path::new(&args[2]);
            marked(root)?;
            let src = Path::new(&args[3]);
            let dst = Path::new(&args[4]);
            if !src.starts_with(root) || !dst.starts_with(root) {
                return Err(Error::new(400, "unsafe_fixture", "预检范围超出隔离夹具"));
            }
            permissions::require_move_permissions(src, dst)?;
            Ok(json!({"ok":true}))
        }
        _ => Err(Error::new(
            400,
            "invalid_request",
            "storage-probe exercise MARKED_ROOT | check MARKED_ROOT SOURCE TARGET",
        )),
    }
}
fn main() {
    match run() {
        Ok(v) => println!("{v}"),
        Err(e) => {
            println!("{}", serde_json::to_string(&e).unwrap());
            std::process::exit(1)
        }
    }
}
