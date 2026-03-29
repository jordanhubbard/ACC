use leptos::*;
use crate::types::{GhIssue, GhIssuesResponse, Project};

async fn fetch_issues(repo: String, state: String) -> GhIssuesResponse {
    let url = if repo.is_empty() {
        format!("/api/issues?state={}&limit=100", state)
    } else {
        format!("/api/issues?repo={}&state={}&limit=100", repo, state)
    };
    let Ok(resp) = gloo_net::http::Request::get(&url).send().await else {
        return GhIssuesResponse::default();
    };
    resp.json::<GhIssuesResponse>().await.unwrap_or_default()
}

async fn fetch_projects() -> Vec<Project> {
    let Ok(resp) = gloo_net::http::Request::get("/api/projects").send().await else {
        return vec![];
    };
    resp.json::<Vec<Project>>().await.unwrap_or_default()
}

async fn trigger_sync(repo: String) -> bool {
    let body = if repo.is_empty() {
        "{}".to_string()
    } else {
        format!(r#"{{"repo":"{}","state":"all"}}"#, repo)
    };
    let Ok(resp) = gloo_net::http::Request::post("/api/issues/sync")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer wq-5dcad756f6d3e345c00b5cb3dfcbdedb")
        .body(body)
        .unwrap()
        .send()
        .await
    else {
        return false;
    };
    resp.status() == 200
}

fn state_badge(state: &str) -> &'static str {
    match state {
        "open" => "badge-open",
        "closed" => "badge-closed",
        _ => "badge-unknown",
    }
}

fn state_icon(state: &str) -> &'static str {
    match state {
        "open" => "🟢",
        "closed" => "⚫",
        _ => "⬜",
    }
}

fn format_date(date: &str) -> String {
    if let Some(d) = date.split('T').next() {
        d.to_string()
    } else {
        date.to_string()
    }
}

fn parse_labels(labels_json: &str) -> Vec<String> {
    serde_json::from_str(labels_json).unwrap_or_default()
}

#[component]
pub fn Issues() -> impl IntoView {
    let (repo_filter, set_repo_filter) = create_signal(String::new());
    let (state_filter, set_state_filter) = create_signal("open".to_string());
    let (tick, set_tick) = create_signal(0u32);
    let (syncing, set_syncing) = create_signal(false);
    let (sync_msg, set_sync_msg) = create_signal(String::new());

    let projects = create_resource(|| (), |_| fetch_projects());

    let issues = create_resource(
        move || (repo_filter.get(), state_filter.get(), tick.get()),
        |(repo, state, _)| fetch_issues(repo, state),
    );

    let on_sync = move |_| {
        let repo = repo_filter.get();
        set_syncing.set(true);
        set_sync_msg.set("Syncing…".to_string());
        leptos::spawn_local(async move {
            let ok = trigger_sync(repo.clone()).await;
            set_syncing.set(false);
            if ok {
                set_sync_msg.set("✅ Sync complete".to_string());
                set_tick.update(|t| *t = t.wrapping_add(1));
            } else {
                set_sync_msg.set("❌ Sync failed".to_string());
            }
        });
    };

    view! {
        <div class="issues-panel">
            <div class="issues-header">
                <h2>"🐛 GitHub Issues"</h2>
                <div class="issues-controls">
                    // Repo selector
                    <select
                        on:change=move |ev| {
                            set_repo_filter.set(event_target_value(&ev));
                            set_tick.update(|t| *t = t.wrapping_add(1));
                        }
                    >
                        <option value="">"All repos"</option>
                        {move || projects.get().unwrap_or_default().iter()
                            .filter(|p| p.enabled.unwrap_or(true))
                            .map(|p| {
                                let id = p.id.clone();
                                let name = p.display_name.clone().unwrap_or_else(|| p.id.clone());
                                view! { <option value={id}>{name}</option> }
                            })
                            .collect::<Vec<_>>()
                        }
                    </select>

                    // State toggle
                    <div class="state-toggle">
                        <button
                            class="toggle-btn"
                            class:toggle-active=move || state_filter.get() == "open"
                            on:click=move |_| {
                                set_state_filter.set("open".to_string());
                                set_tick.update(|t| *t = t.wrapping_add(1));
                            }
                        >"🟢 Open"</button>
                        <button
                            class="toggle-btn"
                            class:toggle-active=move || state_filter.get() == "closed"
                            on:click=move |_| {
                                set_state_filter.set("closed".to_string());
                                set_tick.update(|t| *t = t.wrapping_add(1));
                            }
                        >"⚫ Closed"</button>
                        <button
                            class="toggle-btn"
                            class:toggle-active=move || state_filter.get() == "all"
                            on:click=move |_| {
                                set_state_filter.set("all".to_string());
                                set_tick.update(|t| *t = t.wrapping_add(1));
                            }
                        >"All"</button>
                    </div>

                    // Sync button
                    <button
                        class="sync-btn"
                        disabled=move || syncing.get()
                        on:click=on_sync
                    >
                        {move || if syncing.get() { "⏳ Syncing…" } else { "🔄 Sync GH" }}
                    </button>
                    {move || if !sync_msg.get().is_empty() {
                        view! { <span class="sync-msg">{sync_msg.get()}</span> }.into_view()
                    } else {
                        view! { <span></span> }.into_view()
                    }}
                </div>
            </div>

            // Last sync info
            {move || {
                let data = issues.get().unwrap_or_default();
                if let Some(sync) = data.last_sync {
                    let ts = sync.synced_at.unwrap_or_default();
                    let count = sync.count.unwrap_or(0);
                    view! {
                        <div class="sync-info">
                            {format!("Last sync: {} — {} issues cached", &ts[..ts.len().min(16)], count)}
                        </div>
                    }.into_view()
                } else {
                    view! { <div class="sync-info">"No sync data yet — click Sync GH"</div> }.into_view()
                }
            }}

            // Issue count
            {move || {
                let data = issues.get().unwrap_or_default();
                view! {
                    <div class="issue-count">
                        {format!("{} issue(s)", data.issues.len())}
                    </div>
                }
            }}

            // Issue list
            <div class="issues-list">
                {move || {
                    let data = issues.get().unwrap_or_default();
                    if data.issues.is_empty() {
                        view! {
                            <div class="issues-empty">
                                "No issues found. Try syncing or changing filters."
                            </div>
                        }.into_view()
                    } else {
                        data.issues.iter().map(|issue| {
                            let issue = issue.clone();
                            let labels: Vec<String> = issue.labels.as_deref()
                                .map(parse_labels)
                                .unwrap_or_default();
                            let url = issue.url.clone().unwrap_or_default();
                            let date = issue.updated_at.as_deref()
                                .or(issue.created_at.as_deref())
                                .map(format_date)
                                .unwrap_or_default();
                            let has_wq = issue.wq_id.is_some();

                            view! {
                                <div class="issue-row">
                                    <div class="issue-main">
                                        <span class="issue-icon">{state_icon(&issue.state)}</span>
                                        <a
                                            href={url}
                                            target="_blank"
                                            rel="noopener noreferrer"
                                            class="issue-title"
                                        >
                                            {format!("#{} {}", issue.id, issue.title)}
                                        </a>
                                        {if has_wq {
                                            let wq = issue.wq_id.clone().unwrap_or_default();
                                            view! {
                                                <span class="wq-badge" title={wq.clone()}>
                                                    "🔗 WQ"
                                                </span>
                                            }.into_view()
                                        } else {
                                            view! { <span></span> }.into_view()
                                        }}
                                    </div>
                                    <div class="issue-meta">
                                        <span class="issue-repo">{issue.repo.clone()}</span>
                                        {labels.iter().map(|l| {
                                            let l = l.clone();
                                            view! { <span class="issue-label">{l}</span> }
                                        }).collect::<Vec<_>>()}
                                        <span class="issue-date">{date}</span>
                                        {issue.author.as_ref().map(|a| {
                                            view! { <span class="issue-author">{format!("@{}", a)}</span> }
                                        })}
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>().into_view()
                    }
                }}
            </div>
        </div>
    }
}
