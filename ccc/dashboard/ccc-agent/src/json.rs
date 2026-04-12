/// JSON extraction helpers — replaces all `node -e "JSON.parse(...)"` patterns
/// in the deploy scripts.
///
/// Reads JSON from stdin. Path syntax: `.key`, `.key.subkey`, `.key.subkey.leaf`
///
/// Subcommands:
///   get  <path>                       — print scalar value (string/number/bool)
///   lines <path>                      — print array elements one per line
///   pairs <path>                      — print object as `key=value` lines
///   env-merge <path> <file>           — merge flat-string object into an .env file
///     [--skip=KEY1,KEY2,...]          — keys to skip (default: identity keys)

use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;

// ── Subcommand dispatch ────────────────────────────────────────────────────

pub fn run(args: &[String]) {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "get"       => cmd_get(&args[1..]),
        "lines"     => cmd_lines(&args[1..]),
        "pairs"     => cmd_pairs(&args[1..]),
        "env-merge" => cmd_env_merge(&args[1..]),
        _ => {
            eprintln!("Usage: ccc-agent json <get|lines|pairs|env-merge> <path> [args]");
            std::process::exit(1);
        }
    }
}

// ── Shared: read stdin + navigate path ────────────────────────────────────

fn read_stdin() -> Value {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).unwrap_or(0);
    serde_json::from_str(&buf).unwrap_or(Value::Null)
}

/// Navigate a dotted path like `.secrets.SLACK_BOT_TOKEN`.
/// Returns Null if any segment is missing.
fn navigate<'a>(mut v: &'a Value, path: &str) -> &'a Value {
    // Strip leading dot, split on remaining dots
    let path = path.trim_start_matches('.');
    if path.is_empty() {
        return v;
    }
    for segment in path.split('.') {
        match v.get(segment) {
            Some(next) => v = next,
            None => return &Value::Null,
        }
    }
    v
}

fn scalar_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b)   => Some(b.to_string()),
        Value::Null      => None,
        _                => None,
    }
}

// ── get ───────────────────────────────────────────────────────────────────
//
// ccc-agent json get <path>
//
// Prints the scalar value at <path>. Exits 0 on success, 1 if not found/not scalar.
// Multiple paths can be given; the first non-empty result wins (fallback pattern).
//
// Examples:
//   echo "$JSON" | ccc-agent json get .agentToken
//   echo "$JSON" | ccc-agent json get .secrets.NVIDIA_API_KEY .secrets.nvidia_api_key

fn cmd_get(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: ccc-agent json get <path> [fallback-path ...]");
        std::process::exit(2);
    }
    let root = read_stdin();
    for path in args {
        let node = navigate(&root, path);
        if let Some(s) = scalar_str(node) {
            if !s.is_empty() {
                print!("{s}");
                return;
            }
        }
    }
    // Not found / empty — exit 1 (caller can handle)
    std::process::exit(1);
}

// ── lines ─────────────────────────────────────────────────────────────────
//
// ccc-agent json lines <path>
//
// Prints each element of the array at <path>, one per line.
// Used for: `echo "$RESP" | ccc-agent json lines .keys`

fn cmd_lines(args: &[String]) {
    let path = args.first().unwrap_or_else(|| {
        eprintln!("Usage: ccc-agent json lines <path>");
        std::process::exit(2);
    });
    let root = read_stdin();
    let node = navigate(&root, path);
    match node {
        Value::Array(arr) => {
            for item in arr {
                if let Some(s) = scalar_str(item) {
                    println!("{s}");
                }
            }
        }
        _ => {
            eprintln!("Value at '{path}' is not an array");
            std::process::exit(1);
        }
    }
}

// ── pairs ─────────────────────────────────────────────────────────────────
//
// ccc-agent json pairs <path>
//
// Prints each key=value for the object at <path>, one per line.
// Only flat string/number/bool values are emitted (nested objects are skipped).
// Used for: `echo "$RESP" | ccc-agent json pairs .secrets`

fn cmd_pairs(args: &[String]) {
    let path = args.first().unwrap_or_else(|| {
        eprintln!("Usage: ccc-agent json pairs <path>");
        std::process::exit(2);
    });
    let root = read_stdin();
    let node = navigate(&root, path);
    match node {
        Value::Object(obj) => {
            for (k, v) in obj {
                if let Some(s) = scalar_str(v) {
                    println!("{k}={s}");
                }
            }
        }
        _ => {
            eprintln!("Value at '{path}' is not an object");
            std::process::exit(1);
        }
    }
}

// ── env-merge ─────────────────────────────────────────────────────────────
//
// ccc-agent json env-merge <path> <env-file> [--skip=KEY1,KEY2,...] [--dry-run]
//
// Merges every flat-string value from the JSON object at <path> into <env-file>.
// Existing lines are updated in-place; new keys are appended.
// Keys that contain non-env-safe characters are silently skipped.
//
// Default skip set: CCC_AGENT_TOKEN CCC_URL AGENT_NAME AGENT_HOST
//
// Used by bootstrap.sh step 8b to write the secrets bundle to ~/.ccc/.env.

const DEFAULT_SKIP: &[&str] = &["CCC_AGENT_TOKEN", "CCC_URL", "AGENT_NAME", "AGENT_HOST"];
const VALID_KEY: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_";

fn is_valid_env_key(k: &str) -> bool {
    if k.is_empty() { return false; }
    let mut chars = k.chars();
    // Must start with letter or underscore
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| VALID_KEY.contains(c))
}

fn cmd_env_merge(args: &[String]) {
    if args.len() < 2 {
        eprintln!("Usage: ccc-agent json env-merge <path> <env-file> [--skip=K1,K2] [--dry-run]");
        std::process::exit(2);
    }
    let path     = &args[0];
    let env_file = &args[1];
    let dry_run  = args.iter().any(|a| a == "--dry-run");

    let mut skip: HashSet<String> = DEFAULT_SKIP.iter().map(|s| s.to_string()).collect();
    for arg in &args[2..] {
        if let Some(val) = arg.strip_prefix("--skip=") {
            for k in val.split(',') {
                skip.insert(k.trim().to_string());
            }
        }
    }

    let root = read_stdin();
    let node = navigate(&root, path);
    let obj = match node {
        Value::Object(o) => o,
        _ => {
            eprintln!("Value at '{path}' is not an object");
            std::process::exit(1);
        }
    };

    // Read existing env file (empty string if missing)
    let existing = std::fs::read_to_string(env_file).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(|l| l.to_string()).collect();
    let mut count = 0usize;

    for (k, v) in obj {
        let val = match scalar_str(v) {
            Some(s) => s,
            None => continue,  // skip objects/arrays
        };
        if skip.contains(k) { continue; }
        if !is_valid_env_key(k) { continue; }

        let new_line = format!("{k}={val}");
        if dry_run {
            println!("  DRY  {k}={}", &val.chars().take(16).collect::<String>());
            count += 1;
            continue;
        }

        // Update existing or append
        let prefix = format!("{k}=");
        if let Some(pos) = lines.iter().position(|l| l.starts_with(&prefix)) {
            lines[pos] = new_line;
        } else {
            lines.push(new_line);
        }
        count += 1;
    }

    if !dry_run && count > 0 {
        let mut content = lines.join("\n");
        content.push('\n');
        std::fs::write(env_file, &content).unwrap_or_else(|e| {
            eprintln!("Failed to write {env_file}: {e}");
            std::process::exit(1);
        });
        // chmod 600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(env_file, std::fs::Permissions::from_mode(0o600)).ok();
        }
    }

    eprintln!("env-merge: {count} key(s) {}",
        if dry_run { "would be written" } else { "written" });
}
