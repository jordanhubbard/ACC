use leptos::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScMessage {
    pub id: Option<String>,
    pub from: Option<String>,
    pub text: Option<String>,
    pub channel: Option<String>,
    pub ts: Option<String>,
    pub mentions: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScAgent {
    pub id: Option<String>,
    pub name: String,
    pub online: Option<bool>,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScProject {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub assignee: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScFile {
    pub name: String,
    pub size: Option<u64>,
    pub created_at: Option<String>,
}

// ─── Constants ───────────────────────────────────────────────────────────────

const CHANNELS: &[&str] = &["general", "agents", "ops", "random"];
const FALLBACK_AGENTS: &[&str] = &["natasha", "rocky", "bullwinkle", "sparky", "boris"];

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn format_msg_ts(ts: &str) -> String {
    ts.split('T')
        .nth(1)
        .and_then(|t| t.split('.').next())
        .map(|t| t.trim_end_matches('Z').to_string())
        .unwrap_or_else(|| ts.chars().take(16).collect())
}

fn render_text_with_mentions(text: &str) -> impl IntoView {
    let parts: Vec<leptos::View> = text
        .split(' ')
        .enumerate()
        .map(|(i, word)| {
            let spacer = if i > 0 { " " } else { "" };
            if word.starts_with('@') {
                view! { <span class="sc-mention">{format!("{}{}", spacer, word)}</span> }
                    .into_view()
            } else {
                view! { <span>{format!("{}{}", spacer, word)}</span> }.into_view()
            }
        })
        .collect();
    view! { <span class="sc-msg-body">{parts}</span> }
}

// ─── Async fetchers ───────────────────────────────────────────────────────────

async fn fetch_sc_messages(channel: String) -> Vec<ScMessage> {
    let url = format!("/sc/api/messages?channel={}&limit=50", channel);
    let Ok(resp) = gloo_net::http::Request::get(&url).send().await else {
        return vec![];
    };
    resp.json::<Vec<ScMessage>>().await.unwrap_or_default()
}

async fn fetch_sc_agents() -> Vec<ScAgent> {
    let Ok(resp) = gloo_net::http::Request::get("/sc/api/agents").send().await else {
        return vec![];
    };
    resp.json::<Vec<ScAgent>>().await.unwrap_or_default()
}

async fn fetch_sc_projects() -> Vec<ScProject> {
    let Ok(resp) = gloo_net::http::Request::get("/sc/api/projects").send().await else {
        return vec![];
    };
    resp.json::<Vec<ScProject>>().await.unwrap_or_default()
}

async fn fetch_sc_files(project_id: String) -> Vec<ScFile> {
    let url = format!("/sc/api/projects/{}/files", project_id);
    let Ok(resp) = gloo_net::http::Request::get(&url).send().await else {
        return vec![];
    };
    resp.json::<Vec<ScFile>>().await.unwrap_or_default()
}

// ─── Send helper (free function — signals are Copy) ───────────────────────────

fn trigger_send(
    input_text: ReadSignal<String>,
    set_input_text: WriteSignal<String>,
    selected_channel: ReadSignal<String>,
    set_messages: WriteSignal<Vec<ScMessage>>,
    sending: ReadSignal<bool>,
    set_sending: WriteSignal<bool>,
    set_mention_query: WriteSignal<Option<String>>,
) {
    let text = input_text.get_untracked().trim().to_string();
    if text.is_empty() || sending.get_untracked() {
        return;
    }
    let channel = selected_channel.get_untracked();
    set_sending.set(true);
    set_input_text.set(String::new());
    set_mention_query.set(None);

    let text_clone = text.clone();
    let channel_clone = channel.clone();

    spawn_local(async move {
        #[derive(Serialize)]
        struct SendPayload {
            from: String,
            text: String,
            channel: String,
        }
        let payload = serde_json::json!({
            "from": "jkh",
            "text": text_clone,
            "channel": channel_clone,
        });
        if let Ok(req) = gloo_net::http::Request::post("/sc/api/messages").json(&payload) {
            let _ = req.send().await;
        }
        set_messages.update(|msgs| {
            msgs.push(ScMessage {
                id: None,
                from: Some("jkh".into()),
                text: Some(text_clone),
                channel: Some(channel_clone),
                ts: None,
                mentions: None,
            });
        });
        set_sending.set(false);
    });
}

// ─── Component ────────────────────────────────────────────────────────────────

#[component]
pub fn SquirrelChat() -> impl IntoView {
    let (selected_channel, set_selected_channel) = create_signal("general".to_string());
    let (messages, set_messages) = create_signal(Vec::<ScMessage>::new());
    let (agents, set_agents) = create_signal(Vec::<ScAgent>::new());
    let (projects, set_projects) = create_signal(Vec::<ScProject>::new());
    let (project_files, set_project_files) = create_signal(Vec::<ScFile>::new());
    let (input_text, set_input_text) = create_signal(String::new());
    let (sending, set_sending) = create_signal(false);
    let (selected_project, set_selected_project) = create_signal(Option::<ScProject>::None);
    let (mention_query, set_mention_query) = create_signal(Option::<String>::None);
    let (unread, set_unread) =
        create_signal(std::collections::HashMap::<String, u32>::new());
    let (sc_connected, set_sc_connected) = create_signal(false);
    let (show_new_project, set_show_new_project) = create_signal(false);
    let (new_proj_name, set_new_proj_name) = create_signal(String::new());
    let (new_proj_desc, set_new_proj_desc) = create_signal(String::new());

    let chat_ref = create_node_ref::<leptos::html::Div>();

    // ── Load messages when channel changes ────────────────────────────────────
    let messages_res = create_resource(move || selected_channel.get(), fetch_sc_messages);

    create_effect(move |_| {
        if let Some(msgs) = messages_res.get() {
            set_messages.set(msgs);
        }
    });

    // ── Poll agents every 30 s ────────────────────────────────────────────────
    spawn_local(async move {
        loop {
            let ag = fetch_sc_agents().await;
            if !ag.is_empty() {
                set_agents.set(ag);
            }
            gloo_timers::future::TimeoutFuture::new(30_000).await;
        }
    });

    // ── Load projects once ────────────────────────────────────────────────────
    spawn_local(async move {
        let proj = fetch_sc_projects().await;
        set_projects.set(proj);
    });

    // ── SSE stream for live messages ──────────────────────────────────────────
    {
        if let Ok(es) = web_sys::EventSource::new("/sc/api/stream") {
            let es_cleanup = es.clone();

            let open_cb = Closure::<dyn FnMut()>::new(move || {
                set_sc_connected.set(true);
            });
            es.set_onopen(Some(open_cb.as_ref().unchecked_ref()));
            open_cb.forget();

            let msg_cb = Closure::<dyn FnMut(_)>::new(move |e: web_sys::MessageEvent| {
                let data = e.data().as_string().unwrap_or_default();
                if data.starts_with(':') || data.is_empty() {
                    return;
                }
                #[derive(Deserialize)]
                struct SseEvent {
                    #[serde(rename = "type")]
                    event_type: Option<String>,
                    #[serde(flatten)]
                    msg: ScMessage,
                }
                if let Ok(ev) = serde_json::from_str::<SseEvent>(&data) {
                    if ev.event_type.as_deref() == Some("message") {
                        let ch = ev.msg.channel.clone().unwrap_or_default();
                        let cur = selected_channel.get_untracked();
                        if ch == cur || ch.is_empty() {
                            set_messages.update(|msgs| msgs.push(ev.msg));
                        } else {
                            set_unread.update(|u| {
                                *u.entry(ch).or_insert(0) += 1;
                            });
                        }
                    }
                }
            });
            es.set_onmessage(Some(msg_cb.as_ref().unchecked_ref()));
            msg_cb.forget();

            let err_cb = Closure::<dyn FnMut(_)>::new(move |_: web_sys::ErrorEvent| {
                set_sc_connected.set(false);
            });
            es.set_onerror(Some(err_cb.as_ref().unchecked_ref()));
            err_cb.forget();

            on_cleanup(move || {
                es_cleanup.close();
            });
        }
    }

    // ── Auto-scroll to bottom on new messages ─────────────────────────────────
    create_effect(move |_| {
        let _ = messages.get();
        if let Some(el) = chat_ref.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });

    // ── @mention autocomplete ─────────────────────────────────────────────────
    let mention_suggestions = create_memo(move |_| {
        if let Some(q) = mention_query.get() {
            let q_lower = q.to_lowercase();
            let from_agents: Vec<String> = agents
                .get()
                .into_iter()
                .map(|a| a.name)
                .filter(|n| n.to_lowercase().starts_with(&q_lower))
                .collect();
            if from_agents.is_empty() {
                FALLBACK_AGENTS
                    .iter()
                    .filter(|n| n.to_lowercase().starts_with(&q_lower))
                    .map(|s| s.to_string())
                    .collect()
            } else {
                from_agents
            }
        } else {
            vec![]
        }
    });

    // ── View ──────────────────────────────────────────────────────────────────
    view! {
        <div class="sc-layout">
            // ── Sidebar ────────────────────────────────────────────────────────
            <aside class="sc-sidebar">
                // Channels
                <div class="sc-sidebar-section">
                    <div class="sc-section-header">"Channels"</div>
                    {CHANNELS.iter().map(|&ch| {
                        let ch_str = ch.to_string();
                        view! {
                            <div
                                class="sc-channel-item"
                                class:sc-channel-active=move || selected_channel.get() == ch
                                on:click=move |_| {
                                    set_selected_channel.set(ch_str.clone());
                                    set_unread.update(|u| { u.remove(&ch_str.clone()); });
                                    set_selected_project.set(None);
                                }
                            >
                                <span>"#" {ch}</span>
                                {move || {
                                    let n = unread.get().get(ch).copied().unwrap_or(0);
                                    if n > 0 {
                                        view! { <span class="sc-unread">{n}</span> }.into_view()
                                    } else {
                                        ().into_view()
                                    }
                                }}
                            </div>
                        }
                    }).collect::<Vec<_>>()}
                </div>

                // Agents
                <div class="sc-sidebar-section">
                    <div class="sc-section-header">"Agents"</div>
                    {move || {
                        let ag_list = agents.get();
                        if ag_list.is_empty() {
                            FALLBACK_AGENTS.iter().map(|&name| {
                                view! {
                                    <div class="sc-agent-item">
                                        <span class="sc-presence">"🔴"</span>
                                        <span class="sc-agent-name">{name}</span>
                                    </div>
                                }
                            }).collect::<Vec<_>>().into_view()
                        } else {
                            ag_list.into_iter().map(|a| {
                                let icon = match a.online {
                                    Some(true) => match a.status.as_deref() {
                                        Some("idle") => "🟡",
                                        _ => "🟢",
                                    },
                                    _ => "🔴",
                                };
                                view! {
                                    <div class="sc-agent-item">
                                        <span class="sc-presence">{icon}</span>
                                        <span class="sc-agent-name">{a.name.clone()}</span>
                                    </div>
                                }
                            }).collect::<Vec<_>>().into_view()
                        }
                    }}
                </div>

                // Projects
                <div class="sc-sidebar-section">
                    <div class="sc-section-header">
                        "📁 Projects"
                        <button
                            class="sc-mini-btn"
                            on:click=move |_| set_show_new_project.set(true)
                        >"+"</button>
                    </div>
                    {move || {
                        projects.get().into_iter().map(|p| {
                            let p_click = p.clone();
                            view! {
                                <div
                                    class="sc-project-item"
                                    on:click=move |_| {
                                        let pid = p_click.id.clone();
                                        set_selected_project.set(Some(p_click.clone()));
                                        spawn_local(async move {
                                            let files = fetch_sc_files(pid).await;
                                            set_project_files.set(files);
                                        });
                                    }
                                >
                                    {p.name.clone()}
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}
                </div>
            </aside>

            // ── Main area ─────────────────────────────────────────────────────
            <div class="sc-main">
                {move || {
                    if let Some(proj) = selected_project.get() {
                        // ── Project panel ──────────────────────────────────────
                        let proj_id_dl = proj.id.clone();
                        let proj_id_del = proj.id.clone();
                        let proj_name_del = proj.name.clone();
                        view! {
                            <div class="sc-project-panel">
                                <div class="sc-panel-header">
                                    <h3 class="sc-panel-title">{proj.name.clone()}</h3>
                                    <button
                                        class="sc-close-btn"
                                        on:click=move |_| set_selected_project.set(None)
                                    >"✕ Close"</button>
                                </div>
                                <div class="sc-project-details">
                                    {proj.description.as_deref().map(|d| view! {
                                        <p class="sc-proj-desc">{d.to_string()}</p>
                                    })}
                                    <div class="sc-proj-meta">
                                        {proj.status.as_deref().map(|s| view! {
                                            <span class="sc-meta-item">
                                                <span class="sc-meta-label">"status: "</span>
                                                {s.to_string()}
                                            </span>
                                        })}
                                        {proj.assignee.as_deref().map(|a| view! {
                                            <span class="sc-meta-item">
                                                <span class="sc-meta-label">"assignee: "</span>
                                                {a.to_string()}
                                            </span>
                                        })}
                                    </div>
                                    {proj.tags.as_ref().map(|tags| {
                                        let tag_views: Vec<_> = tags.iter().map(|t| view! {
                                            <span class="sc-tag">{t.clone()}</span>
                                        }).collect();
                                        view! { <div class="sc-tags">{tag_views}</div> }
                                    })}
                                </div>

                                // Files
                                <div class="sc-files-section">
                                    <div class="sc-files-header">
                                        <span>"Files"</span>
                                        <a
                                            class="sc-dl-btn"
                                            href=format!("/sc/api/projects/{}/download", proj_id_dl)
                                            target="_blank"
                                        >"⬇ Download all"</a>
                                    </div>
                                    {move || {
                                        let files = project_files.get();
                                        if files.is_empty() {
                                            view! { <div class="sc-empty-files">"No files"</div> }.into_view()
                                        } else {
                                            files.into_iter().map(|f| view! {
                                                <div class="sc-file-item">
                                                    <span class="sc-file-name">{f.name.clone()}</span>
                                                    {f.size.map(|s| view! {
                                                        <span class="sc-file-size">{format!("{} B", s)}</span>
                                                    })}
                                                </div>
                                            }).collect::<Vec<_>>().into_view()
                                        }
                                    }}
                                    // Upload
                                    <label class="sc-upload-btn">
                                        "📎 Upload file"
                                        <input
                                            type="file"
                                            style="display:none"
                                            on:change=move |ev| {
                                                use wasm_bindgen::JsCast;
                                                let input = ev
                                                    .target()
                                                    .unwrap()
                                                    .unchecked_into::<web_sys::HtmlInputElement>();
                                                let file_list = input.files().unwrap();
                                                if let Some(file) = file_list.get(0) {
                                                    let proj_id = proj.id.clone();
                                                    let file_name = file.name();
                                                    let reader = web_sys::FileReader::new().unwrap();
                                                    let reader2 = reader.clone();
                                                    let onload = Closure::<dyn FnMut(_)>::new(
                                                        move |_: web_sys::Event| {
                                                            if let Ok(result) = reader2.result() {
                                                                if let Some(data_url) = result.as_string() {
                                                                    let b64 = data_url
                                                                        .split(',')
                                                                        .nth(1)
                                                                        .unwrap_or("")
                                                                        .to_string();
                                                                    let pid = proj_id.clone();
                                                                    let fname = file_name.clone();
                                                                    spawn_local(async move {
                                                                        let url = format!(
                                                                            "/sc/api/projects/{}/files",
                                                                            pid
                                                                        );
                                                                        let payload = serde_json::json!({
                                                                            "filename": fname,
                                                                            "content": b64,
                                                                            "encoding": "base64",
                                                                        });
                                                                        if let Ok(req) =
                                                                            gloo_net::http::Request::post(&url)
                                                                                .json(&payload)
                                                                        {
                                                                            let _ = req.send().await;
                                                                        }
                                                                        let files =
                                                                            fetch_sc_files(pid).await;
                                                                        set_project_files.set(files);
                                                                    });
                                                                }
                                                            }
                                                        },
                                                    );
                                                    reader.set_onload(Some(
                                                        onload.as_ref().unchecked_ref(),
                                                    ));
                                                    onload.forget();
                                                    let _ = reader.read_as_data_url(&file);
                                                }
                                            }
                                        />
                                    </label>
                                </div>

                                // Delete project
                                <button
                                    class="sc-delete-btn"
                                    on:click=move |_| {
                                        let pid = proj_id_del.clone();
                                        let pname = proj_name_del.clone();
                                        spawn_local(async move {
                                            let url = format!("/sc/api/projects/{}", pid);
                                            if let Ok(req) =
                                                gloo_net::http::Request::delete(&url).build()
                                            {
                                                let _ = req.send().await;
                                            }
                                            let projs = fetch_sc_projects().await;
                                            set_projects.set(projs);
                                            set_selected_project.set(None);
                                            let _ = pname;
                                        });
                                    }
                                >"🗑 Delete Project"</button>
                            </div>
                        }.into_view()
                    } else {
                        // ── Chat view ──────────────────────────────────────────
                        view! {
                            <div class="sc-chat">
                                <div class="sc-chat-header">
                                    <span class="sc-channel-title">"#" {move || selected_channel.get()}</span>
                                    <span class="sc-conn-badge">
                                        {move || if sc_connected.get() {
                                            view! { <span class="conn-badge conn-live">"● live"</span> }.into_view()
                                        } else {
                                            view! { <span class="conn-badge conn-waiting">"○ offline"</span> }.into_view()
                                        }}
                                    </span>
                                </div>

                                <div class="sc-messages" node_ref=chat_ref>
                                    {move || {
                                        let msgs = messages.get();
                                        if msgs.is_empty() {
                                            return view! {
                                                <div class="sc-no-messages">
                                                    "No messages yet — waiting..."
                                                </div>
                                            }.into_view();
                                        }
                                        msgs.into_iter().enumerate().map(|(i, msg)| {
                                            let ts = msg.ts.as_deref()
                                                .map(format_msg_ts)
                                                .unwrap_or_default();
                                            let from = msg.from.clone()
                                                .unwrap_or_else(|| "?".to_string());
                                            let text = msg.text.clone().unwrap_or_default();
                                            view! {
                                                <div class="sc-msg">
                                                    <div class="sc-msg-header">
                                                        <span class="sc-msg-from">{from}</span>
                                                        <span class="sc-msg-ts">{ts}</span>
                                                        <div class="sc-msg-actions">
                                                            <button class="sc-react-btn" on:click=move |_| {}>"👍"</button>
                                                            <button class="sc-del-btn" on:click=move |_| {
                                                                set_messages.update(|msgs| {
                                                                    if i < msgs.len() { msgs.remove(i); }
                                                                });
                                                            }>"🗑"</button>
                                                        </div>
                                                    </div>
                                                    <div class="sc-msg-content">
                                                        {render_text_with_mentions(&text)}
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>().into_view()
                                    }}
                                </div>

                                <div class="sc-input-area">
                                    {move || {
                                        let suggestions = mention_suggestions.get();
                                        if suggestions.is_empty() {
                                            ().into_view()
                                        } else {
                                            view! {
                                                <div class="sc-mention-dropdown">
                                                    {suggestions.into_iter().map(|name| {
                                                        let n = name.clone();
                                                        view! {
                                                            <div
                                                                class="sc-mention-item"
                                                                on:click=move |_| {
                                                                    set_input_text.update(|t| {
                                                                        if let Some(pos) = t.rfind('@') {
                                                                            t.truncate(pos);
                                                                            t.push('@');
                                                                            t.push_str(&n);
                                                                            t.push(' ');
                                                                        }
                                                                    });
                                                                    set_mention_query.set(None);
                                                                }
                                                            >{name.clone()}</div>
                                                        }
                                                    }).collect::<Vec<_>>()}
                                                </div>
                                            }.into_view()
                                        }
                                    }}
                                    <div class="sc-input-row">
                                        <textarea
                                            class="sc-textarea"
                                            placeholder="Type a message... @mention supported  (Ctrl+Enter to send)"
                                            prop:value=move || input_text.get()
                                            on:input=move |ev| {
                                                let val = event_target_value(&ev);
                                                // Detect @mention
                                                if let Some(at_pos) = val.rfind('@') {
                                                    let after = &val[at_pos + 1..];
                                                    if !after.contains(' ') && !after.is_empty() {
                                                        set_mention_query.set(Some(after.to_string()));
                                                    } else {
                                                        set_mention_query.set(None);
                                                    }
                                                } else {
                                                    set_mention_query.set(None);
                                                }
                                                set_input_text.set(val);
                                            }
                                            on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                if ev.key() == "Enter" && ev.ctrl_key() {
                                                    ev.prevent_default();
                                                    trigger_send(
                                                        input_text, set_input_text,
                                                        selected_channel, set_messages,
                                                        sending, set_sending, set_mention_query,
                                                    );
                                                }
                                            }
                                        />
                                        <button
                                            class="sc-send-btn"
                                            class:sc-sending=move || sending.get()
                                            on:click=move |_| {
                                                trigger_send(
                                                    input_text, set_input_text,
                                                    selected_channel, set_messages,
                                                    sending, set_sending, set_mention_query,
                                                );
                                            }
                                        >
                                            {move || if sending.get() { "Sending..." } else { "Send" }}
                                        </button>
                                    </div>
                                </div>
                            </div>
                        }.into_view()
                    }
                }}
            </div>

            // ── New project modal ─────────────────────────────────────────────
            {move || {
                if !show_new_project.get() {
                    return ().into_view();
                }
                view! {
                    <div class="sc-modal-overlay" on:click=move |_| set_show_new_project.set(false)>
                        <div class="sc-modal" on:click=|ev| ev.stop_propagation()>
                            <h3 class="sc-modal-title">"New Project"</h3>
                            <input
                                type="text"
                                class="sc-modal-input"
                                placeholder="Project name"
                                prop:value=move || new_proj_name.get()
                                on:input=move |ev| set_new_proj_name.set(event_target_value(&ev))
                            />
                            <textarea
                                class="sc-modal-textarea"
                                placeholder="Description (optional)"
                                prop:value=move || new_proj_desc.get()
                                on:input=move |ev| set_new_proj_desc.set(event_target_value(&ev))
                            />
                            <div class="sc-modal-actions">
                                <button class="sc-btn-primary" on:click=move |_| {
                                    let name = new_proj_name.get_untracked();
                                    if name.is_empty() { return; }
                                    let desc = new_proj_desc.get_untracked();
                                    let id = name.to_lowercase().replace(' ', "-");
                                    spawn_local(async move {
                                        let payload = serde_json::json!({
                                            "id": id,
                                            "name": name,
                                            "description": desc,
                                            "tags": [],
                                            "assignee": "jkh",
                                        });
                                        if let Ok(req) = gloo_net::http::Request::post("/sc/api/projects")
                                            .json(&payload) {
                                            let _ = req.send().await;
                                        }
                                        let projs = fetch_sc_projects().await;
                                        set_projects.set(projs);
                                    });
                                    set_show_new_project.set(false);
                                    set_new_proj_name.set(String::new());
                                    set_new_proj_desc.set(String::new());
                                }>"Create"</button>
                                <button class="sc-btn-cancel" on:click=move |_| {
                                    set_show_new_project.set(false);
                                }>"Cancel"</button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            }}
        </div>
    }
}
