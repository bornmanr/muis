//! Theme pack files: `<config>/muis/themes/*.json`.
//!
//! Shared by the native window and the browser hosts so all three read the
//! same directory with the same validation. A bad file is skipped rather than
//! hiding the rest: one typo must not empty the theme list.

use std::path::Path;

/// One pack, in the shape the frontend expects (`{ name, spec }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePack {
    pub name: String,
    pub accent: String,
    pub accent2: String,
    pub green: String,
    pub prompt_path: String,
}

pub fn packs_in(dir: &Path) -> Vec<ThemePack> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut packs: Vec<ThemePack> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| pack_from_file(&path))
        .collect();
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    packs
}

fn pack_from_file(path: &Path) -> Option<ThemePack> {
    let text = std::fs::read_to_string(path).ok()?;
    let raw: serde_json::Value = serde_json::from_str(&text).ok()?;
    let name = raw.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let color = |key: &str| {
        raw.get(key)
            .and_then(|v| v.as_str())
            .filter(|v| valid_hex(v))
            .map(str::to_string)
    };
    let accent = color("accent")?;
    let accent2 = color("accent2")?;
    let green = color("green")?;
    Some(ThemePack {
        prompt_path: color("promptPath").unwrap_or_else(|| green.clone()),
        name,
        accent,
        accent2,
        green,
    })
}

fn valid_hex(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// The JSON the frontend consumes: `[{ "name": ..., "spec": { ... } }]`.
pub fn packs_json(dir: &Path) -> serde_json::Value {
    serde_json::Value::Array(
        packs_in(dir)
            .into_iter()
            .map(|pack| {
                serde_json::json!({
                    "name": pack.name,
                    "spec": {
                        "accent": pack.accent,
                        "accent2": pack.accent2,
                        "green": pack.green,
                        "promptPath": pack.prompt_path,
                    }
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("muis-themes-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_valid_packs_and_skips_the_rest() {
        let dir = temp_dir("packs");
        std::fs::write(
            dir.join("ember.json"),
            r##"{"name":"ember","accent":"#ec5b2b","accent2":"#ee7948","green":"#7fd88f"}"##,
        )
        .unwrap();
        std::fs::write(
            dir.join("partial.json"),
            r##"{"name":"partial","accent":"#123456"}"##,
        )
        .unwrap();
        std::fs::write(
            dir.join("bad.json"),
            r##"{"name":"bad","accent":"red","accent2":"#123456","green":"#123456"}"##,
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "not a pack").unwrap();

        let packs = packs_in(&dir);
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, "ember");
        assert_eq!(packs[0].prompt_path, "#7fd88f");

        let json = packs_json(&dir);
        assert_eq!(json[0]["name"], "ember");
        assert_eq!(json[0]["spec"]["accent"], "#ec5b2b");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_directory_is_empty_not_an_error() {
        assert!(packs_in(Path::new("/definitely/not/here")).is_empty());
    }
}
