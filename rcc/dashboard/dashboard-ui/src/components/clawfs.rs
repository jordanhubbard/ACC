//! ClawFS — per-agent file sync status + namespace browser (COHERENCE-002).

use leptos::*;
use serde::{Deserialize, Serialize};

const AGENTS: &[&str] = &[
    "rocky", "bullwinkle", "natasha", "boris",
    "peabody", "sherman", "snidely", "dudley",
];

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct FsObject {
    key:           String,
    #[serde(default)]
    size:          u64,
    #[serde(default)]
    last_modified: String,
}

fn filename_part(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

fn fmt_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn fmt_ts_short(ts: &str) -> String {
    ts.get(..16).unwrap_or(ts).replace('T', " ")
}

async fn fetch_agent_files(agent: String) -> Vec<FsObject> {
    let url = format!("/api/fs/list?agent={}", agent);
    let Ok(resp) = gloo_net::http::Request::get(&url).send().await else {
        return vec![];
    };
    resp.json::<Vec<FsObject>>().await.unwrap_or_default()
}

async fn fetch_shared_files() -> Vec<FsObject> {
    let Ok(resp) = gloo_net::http::Request::get("/api/fs/list?prefix=shared/")
        .send()
        .await
    else {
        return vec![];
    };
    resp.json::<Vec<FsObject>>().await.unwrap_or_default()
}

// Fetch all 8 agents serially and return (name, file_count, last_sync, healthy).
async fn fetch_all_statuses() -> Vec<(String, usize, String, bool)> {
    let mut result = Vec::with_capacity(AGENTS.len());
    for agent in AGENTS {
        let files = fetch_agent_files(agent.to_string()).await;
        let last_sync = files
            .iter()
            .map(|f| f.last_modified.as_str())
            .max()
            .unwrap_or("")
            .to_string();
        let healthy = !files.is_empty();
        result.push((agent.to_string(), files.len(), last_sync, healthy));
    }
    result
}

#[component]
pub fn AgentFs() -> impl IntoView {
    // Polling tick — 30 s cadence (same pattern as context.rs / geek_view.rs).
    let (tick, set_tick) = create_signal(0u32);
    leptos::spawn_local(async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(30_000).await;
            set_tick.update(|t| *t = t.wrapping_add(1));
        }
    });

    // ── Sync status panel ────────────────────────────────────────────────────
    let statuses = create_resource(move || tick.get(), |_| fetch_all_statuses());

    // ── File browser ─────────────────────────────────────────────────────────
    let (selected_agent, set_selected_agent) = create_signal(AGENTS[0].to_string());
    let browser_files = create_resource(
        move || (tick.get(), selected_agent.get()),
        |(_, agent)| fetch_agent_files(agent),
    );

    // ── Shared files ─────────────────────────────────────────────────────────
    let shared_files = create_resource(move || tick.get(), |_| fetch_shared_files());

    // ── Modal state ───────────────────────────────────────────────────────────
    let (modal_key, set_modal_key)         = create_signal(Option::<String>::None);
    let (modal_content, set_modal_content) = create_signal(String::new());
    let (modal_loading, set_modal_loading) = create_signal(false);

    // open_file captures only WriteSignal<T> (all Copy) → the closure is Copy.
    let open_file = move |key: String| {
        let k = key.clone();
        set_modal_key.set(Some(key));
        set_modal_loading.set(true);
        set_modal_content.set(String::new());
        leptos::spawn_local(async move {
            let url = format!("/api/fs/get?key={}", k);
            let content = match gloo_net::http::Request::get(&url).send().await {
                Ok(resp) => resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "Error: could not read response".to_string()),
                Err(_) => "Error: request failed".to_string(),
            };
            set_modal_content.set(content);
            set_modal_loading.set(false);
        });
    };

    let close_modal = move |_| set_modal_key.set(None);

    // ── View ─────────────────────────────────────────────────────────────────
    view! {
        <div class="clawfs-container">
            <div class="clawfs-header">
                <h2 class="clawfs-title">"📁 ClawFS"</h2>
                <span class="clawfs-subtitle">"Agent file sync status and namespace browser"</span>
            </div>

            // ── Sync Status Panel ────────────────────────────────────────────
            <section class="clawfs-section">
                <h3 class="clawfs-section-title">"Sync Status"</h3>
                {move || match statuses.get() {
                    None => view! {
                        <div class="clawfs-loading">"Loading sync status…"</div>
                    }.into_view(),
                    Some(list) => {
                        let cards = list.into_iter().map(|(name, count, last_sync, healthy)| {
                            let dot_cls   = if healthy { "afs-dot afs-dot-ok"  } else { "afs-dot afs-dot-err" };
                            let badge_cls = if healthy { "afs-badge afs-badge-ok" } else { "afs-badge afs-badge-err" };
                            let badge_txt = if healthy { "healthy" } else { "no files" };
                            let last      = if last_sync.is_empty() { "—".to_string() } else { fmt_ts_short(&last_sync) };
                            view! {
                                <div class="afs-status-card">
                                    <div class="afs-card-top">
                                        <span class={dot_cls}></span>
                                        <span class="afs-agent-name">{name}</span>
                                        <span class={badge_cls}>{badge_txt}</span>
                                    </div>
                                    <div class="afs-card-stat">"Files: " <strong>{count}</strong></div>
                                    <div class="afs-card-stat afs-card-ts">"Last sync: " {last}</div>
                                </div>
                            }
                        }).collect_view();
                        view! { <div class="afs-status-grid">{cards}</div> }.into_view()
                    }
                }}
            </section>

            // ── File Browser ─────────────────────────────────────────────────
            <section class="clawfs-section">
                <h3 class="clawfs-section-title">"File Browser"</h3>
                <div class="afs-browser-toolbar">
                    <label class="afs-select-label">"Agent:"</label>
                    <select
                        class="afs-agent-select"
                        on:change=move |ev| set_selected_agent.set(event_target_value(&ev))
                    >
                        {AGENTS.iter().map(|a| {
                            let a = *a;
                            view! {
                                <option value={a} selected=move || selected_agent.get() == a>
                                    {a}
                                </option>
                            }
                        }).collect_view()}
                    </select>
                </div>
                {move || match browser_files.get() {
                    None => view! {
                        <div class="clawfs-loading">"Loading files…"</div>
                    }.into_view(),
                    Some(files) if files.is_empty() => view! {
                        <div class="afs-empty">"No files in this namespace."</div>
                    }.into_view(),
                    Some(files) => {
                        let rows = files.into_iter().map(|f| {
                            let key  = f.key.clone();
                            let name = filename_part(&f.key).to_string();
                            let size = fmt_size(f.size);
                            let ts   = fmt_ts_short(&f.last_modified);
                            view! {
                                <tr class="afs-file-row" on:click=move |_| open_file(key.clone())>
                                    <td class="afs-col-name">{name}</td>
                                    <td class="afs-col-size">{size}</td>
                                    <td class="afs-col-ts">{ts}</td>
                                </tr>
                            }
                        }).collect_view();
                        view! {
                            <table class="afs-file-table">
                                <thead>
                                    <tr>
                                        <th>"Filename"</th>
                                        <th>"Size"</th>
                                        <th>"Last Modified"</th>
                                    </tr>
                                </thead>
                                <tbody>{rows}</tbody>
                            </table>
                        }.into_view()
                    }
                }}
            </section>

            // ── Shared Files ─────────────────────────────────────────────────
            <section class="clawfs-section">
                <h3 class="clawfs-section-title">"Shared Files"</h3>
                {move || match shared_files.get() {
                    None => view! {
                        <div class="clawfs-loading">"Loading shared files…"</div>
                    }.into_view(),
                    Some(files) if files.is_empty() => view! {
                        <div class="afs-empty">"No files in shared/ namespace."</div>
                    }.into_view(),
                    Some(files) => {
                        let rows = files.into_iter().map(|f| {
                            let key  = f.key.clone();
                            let name = filename_part(&f.key).to_string();
                            let size = fmt_size(f.size);
                            let ts   = fmt_ts_short(&f.last_modified);
                            view! {
                                <tr class="afs-file-row" on:click=move |_| open_file(key.clone())>
                                    <td class="afs-col-name">{name}</td>
                                    <td class="afs-col-size">{size}</td>
                                    <td class="afs-col-ts">{ts}</td>
                                </tr>
                            }
                        }).collect_view();
                        view! {
                            <table class="afs-file-table">
                                <thead>
                                    <tr>
                                        <th>"Filename"</th>
                                        <th>"Size"</th>
                                        <th>"Last Modified"</th>
                                    </tr>
                                </thead>
                                <tbody>{rows}</tbody>
                            </table>
                        }.into_view()
                    }
                }}
            </section>

            // ── File Content Modal ────────────────────────────────────────────
            {move || modal_key.get().map(|key| view! {
                <div class="afs-modal-overlay" on:click=close_modal>
                    <div class="afs-modal" on:click=|ev| ev.stop_propagation()>
                        <div class="afs-modal-header">
                            <span class="afs-modal-key">{key}</span>
                            <button class="afs-modal-close" on:click=close_modal>"✕"</button>
                        </div>
                        <div class="afs-modal-body">
                            {move || if modal_loading.get() {
                                view! {
                                    <div class="clawfs-loading">"Loading content…"</div>
                                }.into_view()
                            } else {
                                view! {
                                    <pre class="afs-modal-pre">{modal_content.get()}</pre>
                                }.into_view()
                            }}
                        </div>
                    </div>
                </div>
            })}
        </div>
    }
}
