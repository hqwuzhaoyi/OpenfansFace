//! Stable identifiers are independent of current paths. Read once per plan, never per file.
use crate::{
    config::Config,
    db, files,
    model::{Error, Result, hash},
};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
};

pub fn document(config: &Config, name: &str, array: bool) -> Result<(Option<String>, Value)> {
    let path = config.base.join(name);
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(Error::conflict("元数据不能是符号链接").at(path.display().to_string()));
    }
    if fs::metadata(&path).is_ok_and(|m| m.len() > 64 * 1024 * 1024) {
        return Err(
            Error::new(413, "metadata_limit", "元数据超过单文档 64 MiB 安全限制")
                .at(path.display().to_string()),
        );
    }
    let raw = files::read_optional(&path)?;
    let value: Value = raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_else(|| if array { json!([]) } else { json!({}) });
    if (array && !value.is_array()) || (!array && !value.is_object()) {
        return Err(
            Error::new(503, "metadata_contract", "元数据格式不符").at(path.display().to_string())
        );
    }
    Ok((raw, value))
}
pub fn disk(config: &Config, path: &str) -> Option<String> {
    let normal = path.replace('\\', "/");
    if normal.split('/').any(|p| p == ".." || p == ".") {
        return None;
    }
    if normal.starts_with(&format!("{}/", config.media.display())) {
        return Some(normal);
    }
    normal
        .split_once("/media/pikpak/")
        .map(|(_, rest)| format!("{}/pikpak/{rest}", config.media.display()))
}
pub struct SourceIndex {
    by_path: BTreeMap<String, Vec<String>>,
}
impl SourceIndex {
    pub fn load(config: &Config, selected_paths: &[String]) -> Result<Self> {
        let (_, history) = document(config, "archive-history.json", true)?;
        let (_, overrides) = document(config, "video-path-overrides.json", false)?;
        let historical: BTreeMap<&str, &str> = history
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| {
                Some((
                    r["vid"].as_str()?,
                    r["win_dst"].as_str().or(r["dst"].as_str())?,
                ))
            })
            .collect();
        let selected: BTreeSet<&str> = selected_paths.iter().map(String::as_str).collect();
        let mut by_path = BTreeMap::<String, Vec<String>>::new();
        let mut consider = |id: &str, path: &str| {
            // A staged/restored source can have a historical canonical target.
            // Its explicit current override remains the authoritative location.
            let effective = overrides[id]
                .as_str()
                .or_else(|| historical.get(id).copied())
                .unwrap_or(path);
            let candidates: Vec<_> = std::iter::once(effective)
                .chain(overrides[id].is_null().then_some(id))
                .filter_map(|p| disk(config, p))
                .collect();
            if !candidates.iter().any(|p| selected.contains(p.as_str())) {
                return;
            }
            let actual = candidates
                .iter()
                .find(|p| Path::new(p).is_file())
                .or(candidates.first());
            if let Some(actual) = actual.filter(|p| selected.contains(p.as_str())) {
                by_path.entry(actual.clone()).or_default().push(id.into());
            }
        };
        // Stream the existing index: memory grows with selected IDs, not library size.
        let connection = db::open(&config.base.join("faces.db"))?;
        let mut query = connection.prepare("SELECT vid,path FROM videos ORDER BY vid")?;
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let path: Option<String> = row.get(1)?;
            consider(&id, path.as_deref().unwrap_or(&id));
        }
        for (id, path) in overrides.as_object().unwrap() {
            if let Some(path) = path.as_str() {
                consider(id, path);
            }
        }
        for (id, path) in historical.iter() {
            consider(id, path);
        }
        for ids in by_path.values_mut() {
            ids.sort();
            ids.dedup();
        }
        Ok(Self { by_path })
    }
    pub fn ids_for_path(&self, path: &str) -> Vec<String> {
        self.by_path.get(path).cloned().unwrap_or_default()
    }
    /// Whole-directory moves carry attachments but register only actual videos as new IDs.
    pub fn tracked_ids(&self, path: &str, stat: &fs::Metadata) -> Result<Vec<String>> {
        let ids = self.ids_for_path(path);
        if !ids.is_empty() {
            return Ok(ids);
        }
        let extension = Path::new(path)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if [
            "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "ts", "mts", "m2ts",
        ]
        .contains(&extension.as_str())
        {
            self.selected_ids(path, path, stat)
        } else {
            Ok(vec![])
        }
    }
    pub fn selected_ids(
        &self,
        path: &str,
        selected: &str,
        stat: &fs::Metadata,
    ) -> Result<Vec<String>> {
        let mut ids = self.ids_for_path(path);
        if ids.is_empty() {
            let synthetic = format!("file:{}", hash(&json!([path, stat.ino().to_string()])));
            if selected != path && selected != synthetic {
                return Err(Error::conflict("未登记视频的编号与实际文件不匹配").at(path));
            }
            ids.push(synthetic);
        } else if !ids.iter().any(|id| id == selected) {
            return Err(Error::conflict("视频编号与实际文件不匹配").at(path));
        }
        Ok(ids)
    }
}
pub fn canonical(config: &Config, name: &str) -> Result<String> {
    files::validate_name(name)?;
    let (_, aliases) = document(config, "person-renames.json", false)?;
    canonical_in(name, &aliases)
}
/// Resolve against one accepted alias document, avoiding repeated IO for whole-person scope.
pub fn canonical_in(name: &str, aliases: &Value) -> Result<String> {
    files::validate_name(name)?;
    let mut current = name.to_owned();
    let mut seen = BTreeSet::new();
    while let Some(next) = aliases[&current].as_str() {
        if !seen.insert(current.clone()) {
            return Err(Error::conflict("人物别名存在循环，请核对"));
        }
        files::validate_name(next)?;
        current = next.into();
    }
    Ok(current)
}
fn folded(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}
/// True when two person names differ only by letter case or Unicode form.
pub fn same_spelling(a: &str, b: &str) -> bool {
    folded(a) == folded(b)
}
/// Whether this exact name (not a case variant) owns a creators directory.
pub fn has_own_directory(config: &Config, name: &str) -> Result<bool> {
    let entries = match fs::read_dir(config.creators()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_str() == Some(name) {
            let kind = entry.file_type()?;
            return Ok(kind.is_dir() && !kind.is_symlink());
        }
    }
    Ok(false)
}
/// Resolve a person picked for a whole-person operation. A name that still owns
/// residue (directory or registration) stays literal even if an alias now points
/// elsewhere, so that residue can be merged or deleted on its own; otherwise the
/// alias resolves to the current person.
pub fn selected(config: &Config, name: &str) -> Result<String> {
    files::validate_name(name)?;
    if known_person(config, name)? {
        return Ok(name.to_owned());
    }
    canonical(config, name)
}
/// Reuse an existing person whose name differs only by letter case or Unicode
/// form, so registering "MomoYih" does not create a second "momoyih".
pub fn existing_spelling(config: &Config, name: &str) -> Result<String> {
    if known_person(config, name)? {
        return Ok(name.to_owned());
    }
    let key = folded(name);
    // A folder is the strongest evidence of the real spelling; registrations
    // only decide when no folder matches.
    let mut folders = BTreeSet::new();
    match fs::read_dir(config.creators()) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if let Some(candidate) = entry.file_name().to_str()
                    && folded(candidate) == key
                    && entry.file_type()?.is_dir()
                {
                    folders.insert(candidate.to_owned());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut registered = BTreeSet::new();
    let (_, people) = document(config, "people-manual.json", true)?;
    for row in people.as_array().into_iter().flatten() {
        if let Some(candidate) = row["name"].as_str()
            && folded(candidate) == key
        {
            registered.insert(candidate.to_owned());
        }
    }
    let found = if folders.is_empty() { registered } else { folders };
    Ok(if found.len() == 1 {
        found.into_iter().next().unwrap()
    } else {
        name.to_owned()
    })
}
pub fn known_person(config: &Config, name: &str) -> Result<bool> {
    // Exact listing, not a path lookup: case-insensitive storage would
    // otherwise report a spelling variant as owning the real folder.
    if has_own_directory(config, name)? {
        return Ok(true);
    }
    let (_, deleted) = document(config, "people-deleted.json", false)?;
    if !deleted[name].is_null() {
        return Ok(false);
    }
    for (file, array, field) in [
        ("people-manual.json", true, "name"),
        ("integration-queue.json", true, "creator"),
        ("report.json", false, "name"),
    ] {
        let (_, value) = document(config, file, array)?;
        let rows = if array {
            value.as_array()
        } else {
            value["identities"].as_array()
        };
        if rows.is_some_and(|rows| rows.iter().any(|r| r[field] == name)) {
            return Ok(true);
        }
    }
    Ok(false)
}
