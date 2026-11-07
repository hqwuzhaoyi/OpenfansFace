//! Merge-specific precedence. Only semantic fields change; profiles and raw evidence remain intact.
use crate::{config::Config, metadata, model::Plan};
use serde_json::{Value, json};
fn names(plan: &Plan, key: &str) -> Vec<String> {
    plan.execution.as_ref().unwrap().data[key]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
fn rows(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}
fn dedup(rows: impl IntoIterator<Item = Value>, key: impl Fn(&Value) -> Value) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    for row in rows {
        if let Some(i) = out.iter().position(|old| key(old) == key(&row)) {
            out[i] = row;
        } else {
            out.push(row);
        }
    }
    out
}
fn union(a: &Value, b: &Value) -> Value {
    let mut result = rows(a);
    for item in rows(b) {
        if !result.contains(&item) {
            result.push(item);
        }
    }
    json!(result)
}
pub fn transform(value: &Value, plan: &Plan) -> Value {
    fn visit(value: &Value, key: &str, plan: &Plan) -> Value {
        match value {
            Value::Array(items) => {
                let mut out = items.iter().map(|v| visit(v, "", plan)).collect::<Vec<_>>();
                if ["candidates", "matches"].contains(&key)
                    && items.iter().any(|v| {
                        v["name"].as_str().is_some_and(|s| {
                            crate::person_rename::names(plan).iter().any(|n| n == s)
                        })
                    })
                    && out.iter().all(|v| v["name"].is_string())
                {
                    let mut merged: Vec<Value> = vec![];
                    for item in out {
                        if let Some(old) = merged.iter_mut().find(|v| v["name"] == item["name"]) {
                            let previous = old.clone();
                            old.as_object_mut()
                                .unwrap()
                                .extend(item.as_object().unwrap().clone());
                            for key in ["sources", "evidence"] {
                                if previous.get(key).is_some() || item.get(key).is_some() {
                                    old[key] = union(&previous[key], &item[key]);
                                }
                            }
                            for key in ["score", "support"] {
                                if let Some(max) = previous[key]
                                    .as_f64()
                                    .into_iter()
                                    .chain(item[key].as_f64())
                                    .reduce(f64::max)
                                {
                                    old[key] = json!(max);
                                }
                            }
                        } else {
                            merged.push(item)
                        }
                    }
                    out = merged;
                }
                json!(out)
            }
            Value::Object(map) => {
                let mut next = Value::Object(
                    map.iter()
                        .map(|(k, v)| (k.clone(), visit(v, k, plan)))
                        .collect(),
                );
                if value["kind"] == "conflict"
                    && rows(&value["candidates"]).len() > rows(&next["candidates"]).len()
                    && rows(&next["candidates"]).len() == 1
                {
                    next["kind"] = json!("suggested");
                    next["suggested"] = next["candidates"][0]["name"].clone();
                }
                next
            }
            _ => value.clone(),
        }
    }
    visit(&crate::person_rename::transform(value, plan), "", plan)
}
pub fn updated(config: &Config, name: &str, value: &Value, plan: &Plan) -> Value {
    let source = names(plan, "source_names");
    let target = names(plan, "target_names");
    let is_target = |v: &Value| v.as_str().is_some_and(|n| target.iter().any(|t| t == n));
    if name.starts_with("external-profiles/") {
        let field = if name.contains("manual-links") {
            "creator"
        } else {
            "person"
        };
        let original = rows(&value["links"]);
        let mut next = value.clone();
        if !value["links"].is_array() {
            return next;
        }
        let mut unrelated = vec![];
        let mut related = vec![];
        let mut order = (0..original.len()).collect::<Vec<_>>();
        order.sort_by_key(|&i| is_target(&original[i][field]));
        for i in order {
            let mut row = original[i].clone();
            if row[field]
                .as_str()
                .is_some_and(|n| source.iter().chain(&target).any(|s| s == n))
            {
                row[field] = json!(plan.target_person);
                let profile = if row["profile"]["id"].is_null() {
                    row["profile_id"].clone()
                } else {
                    row["profile"]["id"].clone()
                };
                if let Some(index) = related.iter().position(|old: &Value| {
                    let old_id = if old["profile"]["id"].is_null() {
                        &old["profile_id"]
                    } else {
                        &old["profile"]["id"]
                    };
                    old_id == &profile
                }) {
                    if field == "person"
                        && related[index]["confidence"] == "candidate"
                        && row["confidence"] == "reference"
                    {
                        continue;
                    }
                    related[index] = row;
                } else {
                    related.push(row)
                }
            } else {
                unrelated.push(row)
            }
        }
        unrelated.extend(related);
        next["links"] = json!(unrelated);
        return next;
    }
    let mut next = transform(&metadata::transform(config, value, plan), plan);
    match name {
        "person-renames.json" => {
            next = value.clone();
            for name in source.iter().chain(&target) {
                if name != &plan.target_person {
                    next[name] = json!(plan.target_person);
                }
            }
            next.as_object_mut().unwrap().remove(&plan.target_person);
        }
        "template-overrides.json" => {
            let chosen = value
                .get(&plan.target_person)
                .or_else(|| target.iter().find_map(|n| next.get(n)))
                .or_else(|| next.get(&plan.person))
                .or_else(|| source.iter().find_map(|n| next.get(n)))
                .cloned();
            for name in source.iter().chain(&target) {
                next.as_object_mut().unwrap().remove(name);
            }
            if let Some(chosen) = chosen {
                next[&plan.target_person] = chosen;
            }
        }
        "report.json" => {
            let original = rows(&value["identities"]);
            let all = rows(&next["identities"]);
            let related = all
                .iter()
                .filter(|r| r["name"] == plan.target_person)
                .cloned()
                .collect::<Vec<_>>();
            if !related.is_empty() {
                let preferred = original
                    .iter()
                    .position(|r| r["name"] == plan.target_person)
                    .or_else(|| original.iter().position(|r| is_target(&r["name"])));
                let base = preferred
                    .map(|i| all[i].clone())
                    .unwrap_or_else(|| related[0].clone());
                let mut combined = base.clone();
                if combined["rep_thumb"].is_null() || combined["rep_thumb"] == "" {
                    combined["rep_thumb"] = related
                        .iter()
                        .find(|r| !r["rep_thumb"].is_null() && r["rep_thumb"] != "")
                        .map(|r| r["rep_thumb"].clone())
                        .unwrap_or(Value::Null);
                }
                let ordered = related
                    .iter()
                    .filter(|r| **r != base)
                    .chain(std::iter::once(&base));
                let mut aliases = vec![];
                let mut vids = vec![];
                for row in ordered {
                    aliases.extend(rows(&row["aliases"]));
                    vids.extend(rows(&row["vids"]));
                }
                aliases.extend(source.iter().map(|s| json!(s)));
                aliases.extend(
                    target
                        .iter()
                        .filter(|s| *s != &plan.target_person)
                        .map(|s| json!(s)),
                );
                combined["aliases"] = json!(dedup(
                    aliases.into_iter().filter(|v| v != &plan.target_person),
                    Clone::clone
                ));
                combined["vids"] = json!(dedup(vids, |v| v["vid"].clone()));
                let mut identities = all
                    .into_iter()
                    .filter(|r| r["name"] != plan.target_person)
                    .collect::<Vec<_>>();
                identities.push(combined);
                next["identities"] = json!(identities);
            }
        }
        "people-manual.json" => {
            let original = rows(value);
            let all = rows(&next);
            let preferred = original
                .iter()
                .position(|r| r["name"] == plan.target_person)
                .or_else(|| original.iter().position(|r| is_target(&r["name"])));
            let mut chosen=preferred.map(|i|all[i].clone()).or_else(||all.iter().find(|r|r["name"]==plan.target_person).cloned()).unwrap_or_else(||json!({"created_at":plan.execution.as_ref().unwrap().data["timestamp"],"rep_thumb":null}));
            chosen["name"] = json!(plan.target_person);
            chosen["last_used_at"] = plan.execution.as_ref().unwrap().data["timestamp"].clone();
            let mut out = all
                .into_iter()
                .filter(|r| r["name"] != plan.target_person)
                .collect::<Vec<_>>();
            out.push(chosen);
            next = json!(out);
        }
        "person-rejections.json" | "claims-detail.json" | "integration-queue.json" => {
            let field = if name == "integration-queue.json" {
                "creator"
            } else {
                "name"
            };
            let (original, all) = if name == "person-rejections.json" {
                (
                    value
                        .as_object()
                        .unwrap()
                        .values()
                        .cloned()
                        .collect::<Vec<_>>(),
                    next.as_object()
                        .unwrap()
                        .values()
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            } else {
                (rows(value), rows(&next))
            };
            let mut order = (0..all.len()).collect::<Vec<_>>();
            order.sort_by_key(|&i| is_target(&original[i][field]));
            let mut out: Vec<Value> = vec![];
            let mut unrelated = vec![];
            for i in order {
                let row = &all[i];
                if row[field] != plan.target_person {
                    unrelated.push(row.clone());
                    continue;
                }
                if let Some(index) = out.iter().position(|old| old["vid"] == row["vid"]) {
                    if name == "claims-detail.json"
                        && out[index]["s"].as_f64().unwrap_or(0.) > row["s"].as_f64().unwrap_or(0.)
                    {
                        continue;
                    }
                    out[index] = row.clone();
                } else {
                    out.push(row.clone());
                }
            }
            unrelated.extend(out);
            next = if name == "person-rejections.json" {
                Value::Object(
                    unrelated
                        .into_iter()
                        .map(|r| {
                            (
                                serde_json::to_string(&json!([r["name"], r["vid"]])).unwrap(),
                                r,
                            )
                        })
                        .collect(),
                )
            } else {
                json!(unrelated)
            };
        }
        _ => {}
    }
    next
}
