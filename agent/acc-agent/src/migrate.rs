use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Record {
    pub status: String,
    #[serde(rename = "appliedAt")]
    pub applied_at: String,
}

pub type State = HashMap<String, Record>;

pub fn run(args: &[String]) {
    let acc_dir = crate::config::acc_dir();
    let state_path = acc_dir.join("migrations.json");

    match args.first().map(String::as_str) {
        Some("is-applied") => {
            let name = args.get(1).map(String::as_str).unwrap_or("");
            if name.is_empty() {
                eprintln!("Usage: acc-agent migrate is-applied <name>");
                std::process::exit(1);
            }
            let state = load(&state_path);
            let applied = state.get(name).map(|r| r.status == "ok").unwrap_or(false);
            std::process::exit(if applied { 0 } else { 1 });
        }
        Some("record") => {
            let name = args.get(1).cloned().unwrap_or_default();
            let status = args.get(2).map(String::as_str).unwrap_or("ok").to_string();
            if name.is_empty() {
                eprintln!("Usage: acc-agent migrate record <name> <ok|failed>");
                std::process::exit(1);
            }
            let mut state = load(&state_path);
            state.insert(
                name.clone(),
                Record {
                    status: status.clone(),
                    applied_at: chrono::Utc::now().to_rfc3339(),
                },
            );
            save(&state_path, &state);
            eprintln!("Recorded {name} as {status}");
        }
        Some("list") => {
            let migrations_dir = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("deploy/migrations"));
            let state = load(&state_path);
            list_migrations(&migrations_dir, &state);
        }
        _ => {
            eprintln!("Usage: acc-agent migrate <is-applied|record|list> [args...]");
            std::process::exit(1);
        }
    }
}

pub fn load(path: &PathBuf) -> State {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(path: &PathBuf, state: &State) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(path, json);
    }
}

fn list_migrations(dir: &PathBuf, state: &State) {
    let mut scripts: Vec<String> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".sh"))
            .collect(),
        Err(e) => {
            eprintln!("Cannot read migrations dir {}: {e}", dir.display());
            return;
        }
    };
    scripts.sort();
    for script in &scripts {
        let name = script.trim_end_matches(".sh");
        let rec = state.get(name);
        let marker = match rec {
            Some(r) if r.status == "ok" => "✓",
            Some(r) if r.status == "failed" => "✗",
            _ => "·",
        };
        let applied_at = rec.map(|r| r.applied_at.as_str()).unwrap_or("");
        println!("{marker} {script:<55} {applied_at}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str) -> (NamedTempFile, PathBuf) {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        (f, path)
    }

    #[test]
    fn test_load_missing() {
        let path = PathBuf::from("/nonexistent/path/migrations.json");
        let state = load(&path);
        assert!(state.is_empty());
    }

    #[test]
    fn test_load_and_check() {
        let json = r#"{"0001_test": {"status": "ok", "appliedAt": "2026-01-01T00:00:00Z"}}"#;
        let (_f, path) = write_temp(json);
        let state = load(&path);
        assert_eq!(state["0001_test"].status, "ok");
        assert_eq!(state["0001_test"].applied_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn test_record_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("migrations.json");
        let mut state = State::new();
        state.insert(
            "0001_test".into(),
            Record {
                status: "ok".into(),
                applied_at: "2026-01-01T00:00:00Z".into(),
            },
        );
        save(&path, &state);
        let loaded = load(&path);
        assert_eq!(loaded["0001_test"].status, "ok");
    }

    #[test]
    fn test_save_creates_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/migrations.json");
        let state = State::new();
        save(&path, &state); // must not panic
        assert!(path.exists());
    }
}
