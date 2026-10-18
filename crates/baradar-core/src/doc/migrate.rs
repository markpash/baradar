//! Schema migrations (port of `web/src/doc/persist-migrate.ts`).
//!
//! Every stored, imported or shared document goes through [`migrate`]: it upgrades the JSON step
//! by step to [`SCHEMA_VERSION`], then validates it ([`validate_doc`]). Documents written by a
//! newer app are not downgraded: they are validated as if current and opened read-only, so they
//! are never overwritten with data loss.

use serde_json::{Map, Value};

use super::schema::{LabelDoc, SCHEMA_VERSION};
use super::validate::{js, validate_doc};

type Json = Map<String, Value>;

/// `CURRENT_SCHEMA`.
pub const CURRENT_SCHEMA: u32 = SCHEMA_VERSION;

/// Keys never copied from untrusted schema-0 JSON (`UNSAFE_KEYS`).
const UNSAFE_KEYS: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// A migrated, validated document.
#[derive(Debug, Clone, PartialEq)]
pub struct Migrated {
    pub doc: LabelDoc,
    /// Written by a newer app: show it, but never save it back (the first edit forks a copy).
    pub read_only: bool,
    /// The schema the input had, when it was not the current one. Saturates at `u32::MAX`.
    pub migrated_from: Option<u32>,
    /// Repairs made by [`validate_doc`]. (The studio computes but does not surface these.)
    pub notices: Vec<String>,
}

fn map_items(raw: &Json, f: impl Fn(&Json) -> Option<Json>) -> Value {
    match raw.get("items") {
        Some(Value::Array(items)) => Value::Array(
            items
                .iter()
                .map(|item| match item {
                    Value::Object(o) => f(o).map_or_else(|| item.clone(), Value::Object),
                    _ => item.clone(),
                })
                .collect(),
        ),
        other => other.cloned().unwrap_or(Value::Null),
    }
}

/// `MIGRATIONS[n]`: upgrades a schema-`n` object to schema `n + 1`; `None` when there is no
/// step from `n`.
///
/// - 0 → 1: unversioned JSON (an object with `items` but no `schema`). Missing top-level settings
///   are filled from `createDoc({items: []})`; an empty or missing name becomes "Imported label".
/// - 1 → 2: code items get `content: 'text'`, and a boolean `quietZone` becomes
///   `'standard'`/`'none'`. A numeric `moduleDots` is kept.
/// - 2 → 3: fixed-size (number or `{mode:'mm'}`) text gets `clipTall: true`, keeping v1's clipping.
/// - 3 → 4: version bump only (36 mm tape and `tape.model` are new).
pub fn migration_step(n: u32, raw: &Json) -> Option<Json> {
    match n {
        0 => {
            let base = serde_json::to_value(LabelDoc::with_items(Vec::new())).ok()?;
            let Value::Object(mut out) = base else {
                return None;
            };
            for (k, v) in raw {
                if !v.is_null() && !UNSAFE_KEYS.contains(&k.as_str()) {
                    out.insert(k.clone(), v.clone());
                }
            }
            out.insert("schema".into(), Value::from(1));
            if !matches!(out.get("name"), Some(Value::String(s)) if !s.is_empty()) {
                out.insert("name".into(), Value::from("Imported label"));
            }
            Some(out)
        }
        1 => {
            let items = map_items(raw, |item| {
                if item.get("kind").and_then(Value::as_str) != Some("code") {
                    return None;
                }
                let mut out = item.clone();
                out.insert("content".into(), Value::from("text"));
                if let Some(Value::Bool(q)) = item.get("quietZone") {
                    out.insert(
                        "quietZone".into(),
                        Value::from(if *q { "standard" } else { "none" }),
                    );
                }
                Some(out)
            });
            let mut out = raw.clone();
            out.insert("schema".into(), Value::from(2));
            out.insert("items".into(), items);
            Some(out)
        }
        2 => {
            let items = map_items(raw, |item| {
                if item.get("kind").and_then(Value::as_str) != Some("text") {
                    return None;
                }
                let fixed = match item.get("size") {
                    Some(Value::Number(_)) => true,
                    Some(Value::Object(s)) => s.get("mode").and_then(Value::as_str) == Some("mm"),
                    _ => false,
                };
                fixed.then(|| {
                    let mut out = item.clone();
                    out.insert("clipTall".into(), Value::Bool(true));
                    out
                })
            });
            let mut out = raw.clone();
            out.insert("schema".into(), Value::from(3));
            out.insert("items".into(), items);
            Some(out)
        }
        3 => {
            let mut out = raw.clone();
            out.insert("schema".into(), Value::from(4));
            Some(out)
        }
        _ => None,
    }
}

/// Schema version of a raw object (0 = unversioned), or a problem string. `versionOf`.
fn version_of(raw: &Json) -> Result<f64, String> {
    match raw.get("schema") {
        None => {
            if raw.get("items").is_some_and(Value::is_array) {
                Ok(0.0)
            } else {
                Err("not a ptouch label document (no \"schema\" or \"items\")".into())
            }
        }
        Some(s) => match s.as_f64() {
            Some(n) if n.is_finite() && n.fract() == 0.0 && n >= 0.0 => Ok(n),
            _ => Err(format!("invalid schema version {}", js::short_json(s, 40))),
        },
    }
}

/// Upgrades any stored, imported or shared JSON to the current schema, then validates it.
/// Port of `migrate`. Never mutates its input.
pub fn migrate(raw: &Value) -> Result<Migrated, Vec<String>> {
    let Value::Object(raw) = raw else {
        return Err(vec![
            "not a ptouch label document (expected a JSON object)".into(),
        ]);
    };
    let from = version_of(raw).map_err(|p| vec![p])?;
    let from_u32 = if from >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        from as u32
    };

    if from > f64::from(CURRENT_SCHEMA) {
        // A newer app wrote this. Read it as the current schema (additive changes are the norm);
        // the caller must not save it back.
        let mut current = raw.clone();
        current.insert("schema".into(), Value::from(CURRENT_SCHEMA));
        return match validate_doc(&Value::Object(current)) {
            Ok(v) => Ok(Migrated {
                doc: v.doc,
                read_only: true,
                migrated_from: Some(from_u32),
                notices: v.notices,
            }),
            Err(problems) => {
                let mut all = vec![format!(
                    "made by a newer version of ptouch studio (schema {}); please update the app",
                    js::num(from)
                )];
                all.extend(problems);
                Err(all)
            }
        };
    }

    let mut doc = raw.clone();
    for n in from_u32..CURRENT_SCHEMA {
        doc = migration_step(n, &doc)
            .ok_or_else(|| vec![format!("no migration from schema {n} to {}", n + 1)])?;
    }

    let v = validate_doc(&Value::Object(doc))?;
    Ok(Migrated {
        doc: v.doc,
        read_only: false,
        migrated_from: (from_u32 != CURRENT_SCHEMA).then_some(from_u32),
        notices: v.notices,
    })
}
