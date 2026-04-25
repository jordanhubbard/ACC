//! Peer discovery — query the hub for other agents in the cluster.
//!
//! Calls GET /api/agents/names?online=true and returns the list, excluding
//! this agent's own name.  Results are best-effort: the hub's view lags by
//! at most one heartbeat interval (~30s).

use std::time::Duration;
use reqwest::Client;
use crate::config::Config;

/// Return the names of all currently-online peers (excluding self).
pub async fn list_peers(cfg: &Config, client: &Client) -> Vec<String> {
    let url = format!("{}/api/agents/names?online=true", cfg.acc_url);
    let resp = match client
        .get(&url)
        .bearer_auth(&cfg.acc_token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    body["names"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter(|n| *n != cfg.agent_name.as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_mock::{HubMock, HubState};
    use serde_json::json;

    fn test_cfg(url: &str, name: &str) -> Config {
        Config {
            acc_dir: std::path::PathBuf::from("/tmp"),
            acc_url: url.to_string(),
            acc_token: "test-tok".to_string(),
            agent_name: name.to_string(),
            agentbus_token: String::new(),
            pair_programming: false,
            host: String::new(),
            ssh_user: "testuser".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: 22,
        }
    }

    #[tokio::test]
    async fn test_list_peers_returns_others() {
        let mock = HubMock::with_state(HubState {
            agent_names: vec!["boris".into(), "natasha".into(), "bullwinkle".into()],
            ..Default::default()
        }).await;
        let client = Client::new();
        let peers = list_peers(&test_cfg(&mock.url, "boris"), &client).await;
        assert!(!peers.contains(&"boris".to_string()), "must exclude self");
        assert!(peers.contains(&"natasha".to_string()));
        assert!(peers.contains(&"bullwinkle".to_string()));
    }

    #[tokio::test]
    async fn test_list_peers_empty_cluster() {
        let mock = HubMock::with_state(HubState {
            agent_names: vec!["boris".into()],
            ..Default::default()
        }).await;
        let client = Client::new();
        let peers = list_peers(&test_cfg(&mock.url, "boris"), &client).await;
        assert!(peers.is_empty());
    }

    #[tokio::test]
    async fn test_list_peers_hub_unreachable_returns_empty() {
        let cfg = test_cfg("http://127.0.0.1:1", "boris");
        let client = Client::builder().timeout(Duration::from_secs(1)).build().unwrap();
        let peers = list_peers(&cfg, &client).await;
        assert!(peers.is_empty());
    }

    #[tokio::test]
    async fn test_list_peers_no_agents_returns_empty() {
        let mock = HubMock::new().await;
        let client = Client::new();
        let peers = list_peers(&test_cfg(&mock.url, "boris"), &client).await;
        assert!(peers.is_empty());
    }
}
