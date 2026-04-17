use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub acc_dir: PathBuf,
    pub acc_url: String,
    pub acc_token: String,
    pub agent_name: String,
    pub agentbus_token: String,
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/home".into()));
        let acc_dir = if home.join(".acc").exists() {
            home.join(".acc")
        } else {
            home.join(".ccc")
        };

        load_env_file(&acc_dir.join(".env"));

        let acc_url = std::env::var("ACC_URL")
            .or_else(|_| std::env::var("CCC_URL"))
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();

        let acc_token = std::env::var("ACC_AGENT_TOKEN")
            .or_else(|_| std::env::var("CCC_AGENT_TOKEN"))
            .unwrap_or_default();

        let agent_name = std::env::var("AGENT_NAME").unwrap_or_default();

        let agentbus_token = std::env::var("AGENTBUS_TOKEN")
            .or_else(|_| std::env::var("SQUIRRELBUS_TOKEN"))
            .unwrap_or_default();

        if acc_url.is_empty() {
            return Err("ACC_URL not set in environment or ~/.acc/.env".into());
        }

        Ok(Config {
            acc_dir,
            acc_url,
            acc_token,
            agent_name,
            agentbus_token,
        })
    }

    pub fn log_file(&self, name: &str) -> PathBuf {
        self.acc_dir.join("logs").join(format!("{name}.log"))
    }

    pub fn quench_file(&self) -> PathBuf {
        self.acc_dir.join("quench")
    }

    pub fn work_signal_file(&self) -> PathBuf {
        self.acc_dir.join("work-signal")
    }
}

pub fn load_env_file(path: &PathBuf) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim().trim_matches('"').trim_matches('\'');
            if std::env::var(key).is_err() {
                // SAFETY: single-threaded init, no concurrent access
                unsafe { std::env::set_var(key, val) };
            }
        }
    }
}

pub fn acc_dir() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/home".into()));
    if home.join(".acc").exists() {
        home.join(".acc")
    } else {
        home.join(".ccc")
    }
}
