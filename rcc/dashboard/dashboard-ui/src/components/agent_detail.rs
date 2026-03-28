use leptos::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentHistoryEntry {
    pub ts: Option<String>,
    pub event: Option<String>,
    pub detail: Option<String>,
    pub pull_rev: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub gpu: Option<bool>,
    pub rust: Option<bool>,
    pub python: Option<bool>,
    pub wasm: Option<bool>,
    pub inference: Option<bool>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetail {
    pub name: Option<String>,
    pub host: Option<String>,
    pub status: Option<String>,
    pub online: Option<bool>,
    pub decommissioned: Option<bool>,
    pub ts: Option<String>,
    pub model: Option<String>,
    pub pull_rev: Option<String>,
    pub gateway_url: Option<String>,
    pub uptime: Option<u64>,
}

type HistoryVec = Vec<AgentHistoryEntry>;

// ── Fetch helpers ─────────────────────────────────────────────────────────────

async fn fetch_agent_detail(name: String) -> AgentDetail {
    let Ok(resp) = gloo_net::http::Request::get(&format!("/api/agents/{name}"))
        .send()
        .await
    else {
        return AgentDetail::default();
    };
    resp.json::<AgentDetail>().await.unwrap_or_default()
}

async fn fetch_agent_history(name: String) -> HistoryVec {
    let Ok(resp) = gloo_net::http::Request::get(&format!("/api/agents/{name}/history"))
        .send()
        .await
    else {
        return vec![];
    };
    resp.json::<HistoryVec>().await.unwrap_or_default()
}

async fn fetch_agent_capabilities(name: String) -> HashMap<String, serde_json::Value> {
    let Ok(resp) =
        gloo_net::http::Request::get(&format!("/api/agents/{name}/capabilities"))
            .send()
            .await
    else {
        return HashMap::new();
    };
    resp.json::<HashMap<String, serde_json::Value>>()
        .await
        .unwrap_or_default()
}

async fn decommission_agent(name: String) -> bool {
    gloo_net::http::Request::post(&format!("/api/agents/{name}/decommission"))
        .send()
        .await
        .map(|r| r.ok())
        .unwrap_or(false)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn format_ts_short(ts: &str) -> String {
    if let Some(t) = ts.split('T').nth(1) {
        t.split('.').next().unwrap_or(t).to_string()
    } else {
        ts.to_string()
    }
}

fn event_icon(event: &str) -> &'static str {
    match event {
        "heartbeat" => "💓",
        "claim" => "📌",
        "complete" => "✅",
        "fail" => "❌",
        "crash" => "💥",
        "restart" => "🔄",
        "deploy" => "🚀",
        _ => "◦",
    }
}

// ── Agent List for sidebar ────────────────────────────────────────────────────

async fn fetch_all_agents() -> Vec<String> {
    let Ok(resp) = gloo_net::http::Request::get("/api/agents").send().await else {
        return vec![];
    };
    // Try array of strings first, then object keys
    if let Ok(arr) = resp.json::<Vec<String>>().await {
        return arr;
    }
    // Try heartbeats map
    let Ok(resp2) = gloo_net::http::Request::get("/api/heartbeats").send().await else {
        return vec![];
    };
    let map: HashMap<String, serde_json::Value> =
        resp2.json().await.unwrap_or_default();
    let mut names: Vec<String> = map.keys().cloned().collect();
    names.sort();
    names
}

// ── Component ─────────────────────────────────────────────────────────────────

#[component]
pub fn AgentDetail() -> impl IntoView {
    let (selected, set_selected) = create_signal(Option::<String>::None);
    let (tick, set_tick) = create_signal(0u32);
    let (decom_msg, set_decom_msg) = create_signal(Option::<String>::None);

    // Refresh every 30s
    leptos::spawn_local(async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(30_000).await;
            set_tick.update(|t| *t = t.wrapping_add(1));
        }
    });

    let agents = create_resource(move || tick.get(), |_| fetch_all_agents());

    let detail = create_resource(
        move || (selected.get(), tick.get()),
        |(name, _)| async move {
            if let Some(n) = name {
                fetch_agent_detail(n).await
            } else {
                AgentDetail::default()
            }
        },
    );

    let history = create_resource(
        move || selected.get(),
        |name| async move {
            if let Some(n) = name {
                fetch_agent_history(n).await
            } else {
                vec![]
            }
        },
    );

    let caps = create_resource(
        move || selected.get(),
        |name| async move {
            if let Some(n) = name {
                fetch_agent_capabilities(n).await
            } else {
                HashMap::new()
            }
        },
    );

    view! {
        <section class="section section-agent-detail">
            <h2 class="section-title">
                <span class="section-icon">"🤖"</span>
                "Agent Detail"
            </h2>
            <div class="agent-detail-layout">
                // Left: agent list
                <div class="agent-detail-sidebar">
                    <h3 class="sidebar-title">"All Agents"</h3>
                    {move || match agents.get() {
                        None => view! { <p class="loading">"Loading..."</p> }.into_view(),
                        Some(names) if names.is_empty() => {
                            view! { <p class="empty">"No agents"</p> }.into_view()
                        }
                        Some(names) => {
                            names
                                .into_iter()
                                .map(|name| {
                                    let n = name.clone();
                                    let n2 = name.clone();
                                    let is_selected = move || {
                                        selected.get().as_deref() == Some(&n2)
                                    };
                                    view! {
                                        <button
                                            class="agent-list-btn"
                                            class:agent-list-btn-active=is_selected
                                            on:click=move |_| {
                                                set_selected.set(Some(n.clone()));
                                                set_decom_msg.set(None);
                                            }
                                        >
                                            {name.clone()}
                                        </button>
                                    }
                                })
                                .collect::<Vec<_>>()
                                .into_view()
                        }
                    }}
                </div>

                // Right: detail panel
                <div class="agent-detail-panel">
                    {move || {
                        if selected.get().is_none() {
                            return view! {
                                <div class="agent-detail-empty">
                                    <p>"← Select an agent to view details"</p>
                                </div>
                            }
                            .into_view();
                        }
                        let agent_name = selected.get().unwrap_or_default();
                        let agent_name2 = agent_name.clone();
                        view! {
                            <div class="agent-detail-content">
                                // Profile card
                                {move || match detail.get() {
                                    None => {
                                        view! { <div class="loading">"Loading profile..."</div> }
                                            .into_view()
                                    }
                                    Some(d) => {
                                        let online = d.online.unwrap_or(false);
                                        let decom = d.decommissioned.unwrap_or(false);
                                        let status_class = if decom {
                                            "decommissioned"
                                        } else if online {
                                            "online"
                                        } else {
                                            "offline"
                                        };
                                        let status_label = if decom {
                                            "decommissioned"
                                        } else if online {
                                            "online"
                                        } else {
                                            "offline"
                                        };
                                        view! {
                                            <div class=format!("agent-profile-card {status_class}")>
                                                <div class="profile-header">
                                                    <span class=format!(
                                                        "agent-dot dot-{status_class}",
                                                    )></span>
                                                    <h2 class="profile-name">
                                                        {d.name.clone().unwrap_or_else(|| agent_name.clone())}
                                                    </h2>
                                                    <span class=format!(
                                                        "profile-status-badge badge-{status_class}",
                                                    )>
                                                        {status_label}
                                                    </span>
                                                </div>
                                                <div class="profile-meta-grid">
                                                    {d
                                                        .host
                                                        .map(|h| {
                                                            view! {
                                                                <div class="meta-row">
                                                                    <span class="meta-key">"host"</span>
                                                                    <span class="meta-val">{h}</span>
                                                                </div>
                                                            }
                                                        })}
                                                    {d
                                                        .model
                                                        .map(|m| {
                                                            view! {
                                                                <div class="meta-row">
                                                                    <span class="meta-key">"model"</span>
                                                                    <span class="meta-val">{m}</span>
                                                                </div>
                                                            }
                                                        })}
                                                    {d
                                                        .pull_rev
                                                        .map(|r| {
                                                            view! {
                                                                <div class="meta-row">
                                                                    <span class="meta-key">"rev"</span>
                                                                    <span class="meta-val code">{r}</span>
                                                                </div>
                                                            }
                                                        })}
                                                    {d
                                                        .gateway_url
                                                        .map(|u| {
                                                            view! {
                                                                <div class="meta-row">
                                                                    <span class="meta-key">"gateway"</span>
                                                                    <span class="meta-val code">{u}</span>
                                                                </div>
                                                            }
                                                        })}
                                                    {d
                                                        .ts
                                                        .as_deref()
                                                        .map(|ts| {
                                                            let display = format_ts_short(ts);
                                                            view! {
                                                                <div class="meta-row">
                                                                    <span class="meta-key">"last heartbeat"</span>
                                                                    <span class="meta-val">{display}</span>
                                                                </div>
                                                            }
                                                        })}
                                                </div>
                                            </div>
                                        }
                                        .into_view()
                                    }
                                }}

                                // Capabilities
                                <div class="section-sub">
                                    <h3 class="sub-title">"Capabilities"</h3>
                                    {move || match caps.get() {
                                        None => {
                                            view! { <span class="loading">"Loading..."</span> }
                                                .into_view()
                                        }
                                        Some(c) if c.is_empty() => {
                                            view! { <span class="empty">"No capabilities registered"</span> }
                                                .into_view()
                                        }
                                        Some(c) => {
                                            let mut pairs: Vec<(String, String)> = c
                                                .iter()
                                                .map(|(k, v)| {
                                                    (
                                                        k.clone(),
                                                        v.as_str()
                                                            .map(|s| s.to_string())
                                                            .or_else(|| {
                                                                v.as_bool().map(|b| b.to_string())
                                                            })
                                                            .unwrap_or_else(|| v.to_string()),
                                                    )
                                                })
                                                .collect();
                                            pairs.sort_by_key(|(k, _)| k.clone());
                                            view! {
                                                <div class="caps-grid">
                                                    {pairs
                                                        .into_iter()
                                                        .map(|(k, v)| {
                                                            view! {
                                                                <span class="cap-badge">
                                                                    <span class="cap-key">{k}</span>
                                                                    ": "
                                                                    <span class="cap-val">{v}</span>
                                                                </span>
                                                            }
                                                        })
                                                        .collect::<Vec<_>>()
                                                        .into_view()}
                                                </div>
                                            }
                                            .into_view()
                                        }
                                    }}
                                </div>

                                // Activity history
                                <div class="section-sub">
                                    <h3 class="sub-title">"Activity History"</h3>
                                    {move || match history.get() {
                                        None => {
                                            view! { <div class="loading">"Loading history..."</div> }
                                                .into_view()
                                        }
                                        Some(h) if h.is_empty() => {
                                            view! { <div class="empty">"No history recorded yet"</div> }
                                                .into_view()
                                        }
                                        Some(mut h) => {
                                            h.reverse(); // newest first
                                            let shown: Vec<_> = h.into_iter().take(30).collect();
                                            view! {
                                                <div class="history-list">
                                                    {shown
                                                        .into_iter()
                                                        .map(|entry| {
                                                            let icon = entry
                                                                .event
                                                                .as_deref()
                                                                .map(event_icon)
                                                                .unwrap_or("◦");
                                                            let ts_short = entry
                                                                .ts
                                                                .as_deref()
                                                                .map(format_ts_short)
                                                                .unwrap_or_default();
                                                            let ev = entry
                                                                .event
                                                                .clone()
                                                                .unwrap_or_default();
                                                            let detail = entry
                                                                .detail
                                                                .clone()
                                                                .unwrap_or_default();
                                                            let rev = entry
                                                                .pull_rev
                                                                .clone()
                                                                .unwrap_or_default();
                                                            view! {
                                                                <div class="history-entry">
                                                                    <span class="history-icon">{icon}</span>
                                                                    <span class="history-ts">{ts_short}</span>
                                                                    <span class="history-event">{ev}</span>
                                                                    {(!detail.is_empty())
                                                                        .then(|| {
                                                                            view! {
                                                                                <span class="history-detail">
                                                                                    {detail}
                                                                                </span>
                                                                            }
                                                                        })}
                                                                    {(!rev.is_empty())
                                                                        .then(|| {
                                                                            let rev_short = rev[..rev.len().min(7)].to_string();
                                                                            view! {
                                                                                <span class="history-rev code">
                                                                                    {rev_short}
                                                                                </span>
                                                                            }
                                                                        })}
                                                                </div>
                                                            }
                                                        })
                                                        .collect::<Vec<_>>()
                                                        .into_view()}
                                                </div>
                                            }
                                            .into_view()
                                        }
                                    }}
                                </div>

                                // Decommission button
                                <div class="section-sub section-danger">
                                    <h3 class="sub-title">"Actions"</h3>
                                    <button
                                        class="btn btn-danger"
                                        on:click={
                                            let agent_name = agent_name2.clone();
                                            move |_| {
                                                let n = agent_name.clone();
                                                leptos::spawn_local(async move {
                                                    let ok = decommission_agent(n.clone()).await;
                                                    set_decom_msg
                                                        .set(Some(if ok {
                                                            format!("Agent {n} decommissioned.")
                                                        } else {
                                                            "Failed to decommission (not authorized or not found).".to_string()
                                                        }));
                                                    set_tick.update(|t| *t = t.wrapping_add(1));
                                                });
                                            }
                                        }
                                    >
                                        "Decommission Agent"
                                    </button>
                                    {move || {
                                        decom_msg
                                            .get()
                                            .map(|msg| {
                                                view! { <p class="decom-msg">{msg}</p> }
                                            })
                                    }}
                                </div>
                            </div>
                        }
                        .into_view()
                    }}
                </div>
            </div>
        </section>
    }
}
