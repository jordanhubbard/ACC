mod session;
mod slack;
mod telegram;

use std::sync::Arc;
use session::SessionStore;
use crate::config::Config;
use super::agent::HermesAgent;
use super::provider::make_provider;
use super::tool::ToolRegistry;
use acc_client::Client;

/// Run the gateway for a specific workspace.
///
/// `workspace` is an optional uppercase suffix appended to env var names:
///   None          → SLACK_APP_TOKEN,          SLACK_BOT_TOKEN / SLACK_OMGJKH_TOKEN
///   Some("OMGJKH") → SLACK_APP_TOKEN (existing), no change (backward compat)
///   Some("OFTERRA") → SLACK_APP_TOKEN_OFTERRA,  SLACK_BOT_TOKEN_OFTERRA
pub async fn run(workspace: Option<&str>) {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[hermes-gateway] config error: {e}");
            std::process::exit(1);
        }
    };

    let ws_label = workspace.unwrap_or("default");
    eprintln!("[hermes-gateway] starting agent={} hub={} workspace={ws_label}",
        cfg.agent_name, cfg.acc_url);

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .unwrap_or_default();
    let model = std::env::var("HERMES_MODEL")
        .unwrap_or_else(|_| "claude-opus-4-7".to_string());
    let provider = make_provider(api_key, model);
    let tools = ToolRegistry::default_tools();
    let client = Client::new(&cfg.acc_url, &cfg.acc_token).expect("acc client");
    let agent = Arc::new(HermesAgent::new(cfg.clone(), client, provider, tools));

    // Sessions are namespaced by workspace so conversations don't bleed across.
    let sessions_dir = cfg.acc_dir.join("data").join("sessions")
        .join(workspace.unwrap_or("default").to_lowercase());
    let sessions = Arc::new(SessionStore::new(sessions_dir)
        .with_hub(
            Client::new(&cfg.acc_url, &cfg.acc_token).expect("sessions client"),
            cfg.agent_name.clone(),
            workspace.unwrap_or("default").to_lowercase(),
        ));

    let mut handles = Vec::new();

    // Start Slack if configured.
    match slack::SlackAdapter::new(sessions.clone(), agent.clone(), workspace).await {
        Some(adapter) => {
            eprintln!("[hermes-gateway/{ws_label}] Slack adapter started");
            let adapter = Arc::new(adapter);
            handles.push(tokio::spawn(async move { adapter.run().await }));
        }
        None => eprintln!("[hermes-gateway/{ws_label}] Slack not configured (SLACK_APP_TOKEN{} missing)",
            workspace.map(|w| format!("_{w}")).unwrap_or_default()),
    }

    // Start Telegram if configured (only for default workspace — no per-workspace Telegram).
    if workspace.is_none() {
        match telegram::TelegramAdapter::new(sessions.clone(), agent.clone()).await {
            Some(adapter) => {
                eprintln!("[hermes-gateway/default] Telegram adapter started");
                let adapter = Arc::new(adapter);
                handles.push(tokio::spawn(async move { adapter.run().await }));
            }
            None => eprintln!("[hermes-gateway/default] Telegram not configured (TELEGRAM_BOT_TOKEN missing)"),
        }
    }

    if handles.is_empty() {
        eprintln!("[hermes-gateway/{ws_label}] no platforms configured — exiting");
        std::process::exit(1);
    }

    for h in handles {
        let _ = h.await;
    }
}
