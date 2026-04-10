/// Sidebar — workspace header, channel list, DM list, user footer.
use leptos::*;
use crate::types::{ChatContext, DEFAULT_CHANNELS};

fn ls_remove(key: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.remove_item(key);
    }
}

#[component]
pub fn Sidebar() -> impl IntoView {
    let ctx = use_context::<ChatContext>().expect("ChatContext");
    let (show_add, set_show_add) = create_signal(false);
    let (new_channel, set_new_channel) = create_signal(String::new());

    let mark_read = move |ch: String| {
        let count = ctx.messages.get().iter()
            .filter(|m| m.subject.as_deref() == Some(ch.as_str()))
            .count();
        ctx.read_counts.update(|m| { m.insert(ch, count); });
    };

    let channel_unread = move |ch: &str| -> usize {
        let current = ctx.messages.get().iter()
            .filter(|m| m.subject.as_deref() == Some(ch))
            .count();
        let watermark = ctx.read_counts.get().get(ch).copied().unwrap_or(current);
        current.saturating_sub(watermark).min(99)
    };

    view! {
        <div class="chat-sidebar">

            // ── Workspace header ──────────────────────────────────────────
            <div class="workspace-header">
                <div class="workspace-name-row">
                    <span class="workspace-logo">"🦞"</span>
                    <span class="workspace-name">"ClawChat"</span>
                    <span class=move || {
                        if ctx.connected.get() { "conn-pill live" } else { "conn-pill dead" }
                    }>{move || if ctx.connected.get() { "●" } else { "○" }}</span>
                </div>
                // Search / Cmd+K trigger
                <button class="sidebar-search-btn" on:click=move |_| ctx.palette_open.set(true)>
                    <span class="search-icon">"🔍"</span>
                    <span class="search-label">"Search or jump…"</span>
                    <span class="search-shortcut">"⌘K"</span>
                </button>
            </div>

            // ── Channels section ──────────────────────────────────────────
            <div class="sidebar-section">
                <div class="sidebar-section-header">
                    <span class="sidebar-section-label">"CHANNELS"</span>
                    <button class="add-channel-btn" title="Add channel"
                        on:click=move |_| set_show_add.update(|v| *v = !*v)
                    >"+"</button>
                </div>

                // Add-channel inline form
                {move || {
                    if !show_add.get() { return view! { <></> }.into_view(); }
                    view! {
                        <div class="add-channel-form">
                            <input
                                class="add-channel-input"
                                type="text"
                                placeholder="channel-name"
                                prop:value=new_channel
                                on:input=move |e| set_new_channel.set(event_target_value(&e))
                                on:keydown=move |e| {
                                    if e.key() == "Enter" {
                                        let mut name = new_channel.get();
                                        if !name.starts_with('#') { name = format!("#{name}"); }
                                        if !name.trim_start_matches('#').is_empty() {
                                            ctx.active_channel.set(name);
                                        }
                                        set_new_channel.set(String::new());
                                        set_show_add.set(false);
                                    }
                                    if e.key() == "Escape" { set_show_add.set(false); }
                                }
                            />
                        </div>
                    }.into_view()
                }}

                // Default channels
                {DEFAULT_CHANNELS.iter().map(|(ch_id, ch_label)| {
                    let id    = ch_id.to_string();
                    let lbl   = ch_label.to_string();
                    let id_a  = id.clone();
                    let id_c  = id.clone();
                    let id_u  = id.clone();
                    let mr    = mark_read.clone();

                    view! {
                        <button
                            class="channel-item"
                            class:channel-active=move || ctx.active_channel.get() == id_a
                            on:click=move |_| {
                                let ch = id_c.clone();
                                mr(ch.clone());
                                ctx.active_channel.set(ch);
                                ctx.open_thread.set(None);
                            }
                        >
                            <span class="channel-hash">"#"</span>
                            <span class="channel-name">{lbl}</span>
                            {move || {
                                let n = channel_unread(&id_u);
                                if n > 0 { view! { <span class="unread-badge">{n}</span> }.into_view() }
                                else     { view! { <></> }.into_view() }
                            }}
                        </button>
                    }
                }).collect::<Vec<_>>().into_view()}

                // Discovered extra channels
                {move || {
                    ctx.discovered_channels().into_iter().map(|id| {
                        let lbl  = id.trim_start_matches('#').to_string();
                        let id_a = id.clone();
                        let id_c = id.clone();
                        let id_u = id.clone();
                        let mr   = mark_read.clone();
                        view! {
                            <button
                                class="channel-item channel-discovered"
                                class:channel-active=move || ctx.active_channel.get() == id_a
                                on:click=move |_| {
                                    let ch = id_c.clone();
                                    mr(ch.clone());
                                    ctx.active_channel.set(ch);
                                    ctx.open_thread.set(None);
                                }
                            >
                                <span class="channel-hash">"#"</span>
                                <span class="channel-name">{lbl}</span>
                                {move || {
                                    let n = channel_unread(&id_u);
                                    if n > 0 { view! { <span class="unread-badge">{n}</span> }.into_view() }
                                    else     { view! { <></> }.into_view() }
                                }}
                            </button>
                        }
                    }).collect::<Vec<_>>().into_view()
                }}
            </div>

            // ── Direct Messages section ───────────────────────────────────
            <div class="sidebar-section">
                <div class="sidebar-section-header">
                    <span class="sidebar-section-label">"DIRECT MESSAGES"</span>
                </div>

                // DMs from presence
                {move || {
                    let me = ctx.username.get();
                    let mut peers: Vec<String> = ctx.presence.get()
                        .iter()
                        .filter(|p| p.agent != me)
                        .map(|p| p.agent.clone())
                        .collect();
                    // Also include anyone we've DMed
                    for peer in ctx.dm_peers(&me) {
                        if !peers.contains(&peer) { peers.push(peer); }
                    }
                    peers.sort();

                    peers.into_iter().map(|peer| {
                        let dm_id  = format!("dm:{peer}");
                        let dm_a   = dm_id.clone();
                        let dm_c   = dm_id.clone();
                        let peer2  = peer.clone();

                        let online = ctx.presence.get()
                            .iter()
                            .find(|p| p.agent == peer2)
                            .map(|p| p.online)
                            .unwrap_or(false);

                        view! {
                            <button
                                class="dm-item"
                                class:channel-active=move || ctx.active_channel.get() == dm_a
                                on:click=move |_| {
                                    ctx.active_channel.set(dm_c.clone());
                                    ctx.open_thread.set(None);
                                }
                            >
                                <span class=if online { "dm-dot online" } else { "dm-dot offline" } />
                                <span class="dm-name">{peer.clone()}</span>
                            </button>
                        }
                    }).collect::<Vec<_>>().into_view()
                }}
            </div>

            // ── Presence (non-DM agents) ──────────────────────────────────
            <div class="sidebar-section sidebar-presence">
                <div class="sidebar-section-header">
                    <span class="sidebar-section-label">"AGENTS"</span>
                </div>
                {move || {
                    let p = ctx.presence.get();
                    if p.is_empty() {
                        return view! {
                            <div class="presence-empty">"no heartbeats yet"</div>
                        }.into_view();
                    }
                    p.into_iter().map(|e| {
                        view! {
                            <div class="presence-item">
                                <span class=if e.online { "presence-dot online" } else { "presence-dot offline" } />
                                <span class="presence-name">{e.agent}</span>
                            </div>
                        }
                    }).collect::<Vec<_>>().into_view()
                }}
            </div>

            // ── User footer ───────────────────────────────────────────────
            <div class="sidebar-footer">
                <span class="sidebar-footer-user">{move || ctx.username.get()}</span>
                <button class="sidebar-signout-btn" on:click=move |_| {
                    ls_remove("ccc_token");
                    ls_remove("ccc_username");
                    ctx.set_token.set(None);
                }>"Sign out"</button>
            </div>

        </div>
    }
}
