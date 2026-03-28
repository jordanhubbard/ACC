use leptos::*;

use crate::types::{HeartbeatData, HeartbeatMap};

async fn fetch_heartbeats() -> HeartbeatMap {
    let Ok(resp) = gloo_net::http::Request::get("/api/heartbeats").send().await else {
        return HeartbeatMap::default();
    };
    resp.json::<HeartbeatMap>().await.unwrap_or_default()
}

fn format_ts(ts: &str) -> String {
    // Show just the time portion for readability
    if let Some(t) = ts.split('T').nth(1) {
        t.split('.').next().unwrap_or(t).to_string()
    } else {
        ts.to_string()
    }
}

#[component]
pub fn AgentCards() -> impl IntoView {
    let (tick, set_tick) = create_signal(0u32);

    // Poll every 30 seconds
    leptos::spawn_local(async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(30_000).await;
            set_tick.update(|t| *t = t.wrapping_add(1));
        }
    });

    let heartbeats = create_resource(move || tick.get(), |_| fetch_heartbeats());

    view! {
        <section class="section section-agents">
            <h2 class="section-title">
                <span class="section-icon">"●"</span>
                "Agents"
            </h2>
            <div class="agent-grid">
                {move || match heartbeats.get() {
                    None => view! { <p class="loading">"Loading agents..."</p> }.into_view(),
                    Some(agents) if agents.is_empty() => {
                        view! { <p class="empty">"No agents registered"</p> }.into_view()
                    }
                    Some(agents) => {
                        let mut sorted: Vec<(String, HeartbeatData)> =
                            agents.into_iter().collect();
                        sorted.sort_by(|a, b| a.0.cmp(&b.0));
                        sorted
                            .into_iter()
                            .map(|(name, hb)| {
                                let online = hb.online.unwrap_or(false);
                                let decom = hb.decommissioned.unwrap_or(false);
                                let status_class = if decom {
                                    "decommissioned"
                                } else if online {
                                    "online"
                                } else {
                                    "offline"
                                };
                                let ts_display = hb
                                    .ts
                                    .as_deref()
                                    .map(format_ts)
                                    .unwrap_or_else(|| "never".to_string());
                                view! {
                                    <div class=format!("agent-card {status_class}")>
                                        <div class="agent-header">
                                            <span class=format!(
                                                "agent-dot dot-{status_class}",
                                            )></span>
                                            <span class="agent-name">{name}</span>
                                        </div>
                                        <div class="agent-meta">
                                            {hb
                                                .host
                                                .map(|h| {
                                                    view! {
                                                        <span class="meta-item">
                                                            <span class="meta-label">"host:"</span>
                                                            {h}
                                                        </span>
                                                    }
                                                })}
                                            {hb
                                                .model
                                                .map(|m| {
                                                    view! {
                                                        <span class="meta-item">
                                                            <span class="meta-label">"model:"</span>
                                                            {m}
                                                        </span>
                                                    }
                                                })}
                                        </div>
                                        <div class="agent-ts">"last: " {ts_display}</div>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_view()
                    }
                }}
            </div>
        </section>
    }
}
