use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value as YamlValue};

use crate::WorkflowError;

pub(crate) fn get_map<'a>(root: &'a Mapping, key: &str) -> Option<&'a Mapping> {
    root.get(YamlValue::String(key.to_string()))
        .and_then(YamlValue::as_mapping)
}

pub(crate) fn nested_map<'a>(map: Option<&'a Mapping>, key: &str) -> Option<&'a Mapping> {
    map.and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(YamlValue::as_mapping)
}

pub(crate) fn string_value(map: Option<&Mapping>, key: &str) -> Option<String> {
    map.and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(|v| match v {
            YamlValue::String(s) => Some(s.clone()),
            YamlValue::Number(n) => Some(n.to_string()),
            _ => None,
        })
}

pub(crate) fn string_vec(map: Option<&Mapping>, key: &str) -> Option<Vec<String>> {
    map.and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(YamlValue::as_sequence)
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect()
        })
}

pub(crate) fn u64_value(map: Option<&Mapping>, key: &str) -> Option<u64> {
    map.and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(YamlValue::as_u64)
}

pub(crate) fn i64_value(map: Option<&Mapping>, key: &str) -> Option<i64> {
    map.and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(YamlValue::as_i64)
}

pub(crate) fn u32_value(map: Option<&Mapping>, key: &str) -> Option<u32> {
    u64_value(map, key).map(|value| value as u32)
}

pub(crate) fn usize_value(map: Option<&Mapping>, key: &str) -> Option<usize> {
    u64_value(map, key).map(|value| value as usize)
}

pub(crate) fn json_value(map: Option<&Mapping>, key: &str) -> Option<serde_json::Value> {
    map.and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(|value| serde_json::to_value(value).ok())
}

pub(crate) fn concurrency_map(map: Option<&Mapping>, key: &str) -> HashMap<String, usize> {
    map.and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(YamlValue::as_mapping)
        .map(|mapping| {
            mapping
                .iter()
                .filter_map(|(key, value)| {
                    let key = key.as_str()?.to_lowercase();
                    let value = value.as_u64()? as usize;
                    (value > 0).then_some((key, value))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn resolve_exact_env_from(
    raw: String,
    env_map: &HashMap<String, String>,
) -> Result<String, WorkflowError> {
    if let Some(var) = raw.strip_prefix('$') {
        if var.is_empty() || var.contains('/') || var.contains(' ') {
            return Ok(raw);
        }
        Ok(env_map.get(var).cloned().unwrap_or_default())
    } else {
        Ok(raw)
    }
}

pub(crate) fn expand_path_value_from(
    raw: &str,
    base: &Path,
    env_map: &HashMap<String, String>,
) -> Result<PathBuf, WorkflowError> {
    let mut value = raw.to_string();
    if let Some(var) = value.strip_prefix('$')
        && !var.is_empty()
        && !var.contains('/')
        && !var.contains(' ')
    {
        value = env_map.get(var).cloned().unwrap_or_default();
    }
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = env_map.get("HOME")
    {
        value = PathBuf::from(home).join(rest).to_string_lossy().to_string();
    }
    let path = PathBuf::from(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    Ok(absolute)
}
