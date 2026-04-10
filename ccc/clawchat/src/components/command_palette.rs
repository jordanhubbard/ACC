/// Cmd+K command palette — quick channel + DM switcher.
use leptos::*;
use crate::types::{ChatContext, DEFAULT_CHANNELS};

#[derive(Clone, PartialEq)]
struct PaletteItem {
    id: String,
    label: String,
    kind: &'static str, // "channel" | "dm"
}

#[component]
pub fn CommandPalette() -> impl IntoView {
    let ctx = use_context::<ChatContext>().expect("ChatContext");
    let (query, set_query) = create_signal(String::new());
    let (selected, set_selected) = create_signal(0usize);

    // All navigable items (channels + DMs)
    let all_items = create_memo(move |_| {
        let mut items: Vec<PaletteItem> = Vec::new();
        for (id, label) in DEFAULT_CHANNELS {
            items.push(PaletteItem { id: id.to_string(), label: label.to_string(), kind: "channel" });
        }
        for extra in ctx.discovered_channels() {
            let label = extra.trim_start_matches('#').to_string();
            items.push(PaletteItem { id: extra, label, kind: "channel" });
        }
        let me = ctx.username.get();
        for peer in ctx.dm_peers(&me) {
            items.push(PaletteItem { id: format!("dm:{peer}"), label: peer.clone(), kind: "dm" });
        }
        // Also add known agents from presence
        for p in ctx.presence.get() {
            let dm_id = format!("dm:{}", p.agent);
            if !items.iter().any(|i| i.id == dm_id) {
                items.push(PaletteItem { id: dm_id, label: p.agent, kind: "dm" });
            }
        }
        items
    });

    let filtered = create_memo(move |_| {
        let q = query.get().to_lowercase();
        all_items.get()
            .into_iter()
            .filter(|item| q.is_empty() || item.label.to_lowercase().contains(&q))
            .collect::<Vec<_>>()
    });

    let navigate_to = move |id: String| {
        ctx.active_channel.set(id);
        ctx.palette_open.set(false);
        set_query.set(String::new());
        set_selected.set(0);
    };

    view! {
        {move || {
            if !ctx.palette_open.get() {
                return view! { <></> }.into_view();
            }
            let items = filtered.get();
            let count = items.len();

            view! {
                // Full-screen backdrop — click to close
                <div class="palette-overlay" on:click=move |_| {
                    ctx.palette_open.set(false);
                    set_query.set(String::new());
                    set_selected.set(0);
                }>
                    // Modal — stop click from bubbling to overlay
                    <div class="palette-modal" on:click=|e| e.stop_propagation()>
                        <div class="palette-search-row">
                            <span class="palette-search-icon">"⌘"</span>
                            <input
                                class="palette-input"
                                type="text"
                                placeholder="Jump to channel or person…"
                                autofocus=true
                                prop:value=query
                                on:input=move |e| {
                                    set_query.set(event_target_value(&e));
                                    set_selected.set(0);
                                }
                                on:keydown=move |e| {
                                    match e.key().as_str() {
                                        "ArrowDown" => {
                                            e.prevent_default();
                                            set_selected.update(|s| *s = (*s + 1).min(count.saturating_sub(1)));
                                        }
                                        "ArrowUp" => {
                                            e.prevent_default();
                                            set_selected.update(|s| *s = s.saturating_sub(1));
                                        }
                                        "Enter" => {
                                            if let Some(item) = filtered.get().get(selected.get()).cloned() {
                                                navigate_to(item.id);
                                            }
                                        }
                                        "Escape" => {
                                            ctx.palette_open.set(false);
                                            set_query.set(String::new());
                                            set_selected.set(0);
                                        }
                                        _ => {}
                                    }
                                }
                            />
                        </div>

                        <div class="palette-results">
                            {move || {
                                let items = filtered.get();
                                let sel   = selected.get();
                                if items.is_empty() {
                                    return view! {
                                        <div class="palette-empty">"No channels or people found"</div>
                                    }.into_view();
                                }
                                items.into_iter().enumerate().map(|(i, item)| {
                                    let id2  = item.id.clone();
                                    let icon = if item.kind == "dm" { "@" } else { "#" };
                                    let is_active = i == sel;
                                    view! {
                                        <button
                                            class=move || if is_active { "palette-item palette-item-active" } else { "palette-item" }
                                            on:click=move |_| navigate_to(id2.clone())
                                        >
                                            <span class="palette-item-icon">{icon}</span>
                                            <span class="palette-item-label">{item.label.clone()}</span>
                                            <span class="palette-item-kind">{item.kind}</span>
                                        </button>
                                    }
                                }).collect::<Vec<_>>().into_view()
                            }}
                        </div>
                    </div>
                </div>
            }.into_view()
        }}
    }
}
