/// Input bar — compose and send messages with formatting toolbar and topic support.
use leptos::*;
use wasm_bindgen_futures::spawn_local;
use crate::types::{BusMessage, ChatContext};

#[component]
pub fn InputBar() -> impl IntoView {
    let ctx = use_context::<ChatContext>().expect("ChatContext");
    let (text, set_text)   = create_signal(String::new());
    let (topic, set_topic) = create_signal(String::new());
    let (sending, set_sending) = create_signal(false);

    let do_send = move || {
        let body = text.get();
        if body.trim().is_empty() || sending.get() { return; }

        let ch   = ctx.active_channel.get();
        let from = ctx.username.get();
        let tok  = ctx.token.get().unwrap_or_default();
        let tp   = topic.get();
        let set_msgs = ctx.set_messages;

        // Build payload differently for DM vs channel
        let payload = if ctx.is_dm_view() {
            let peer = ctx.dm_peer();
            serde_json::json!({
                "from": from, "to": peer,
                "type": "text", "subject": "dm",
                "body": body, "mime": "text/plain",
            })
        } else {
            let mut p = serde_json::json!({
                "from": from, "to": "all",
                "type": "text", "subject": ch,
                "body": body, "mime": "text/plain",
            });
            if !tp.trim().is_empty() {
                p["topic"] = serde_json::Value::String(tp.trim().to_string());
            }
            p
        };

        set_text.set(String::new());
        set_sending.set(true);

        spawn_local(async move {
            let res = gloo_net::http::Request::post("/bus/send")
                .header("Authorization", &format!("Bearer {tok}"))
                .header("Content-Type", "application/json")
                .body(payload.to_string())
                .unwrap()
                .send()
                .await;
            if let Ok(resp) = res {
                if resp.ok() {
                    if let Ok(msg) = resp.json::<BusMessage>().await {
                        set_msgs.update(|v| v.push(msg));
                    }
                }
            }
            set_sending.set(false);
        });
    };

    /// Wrap selected text (or insert placeholder) with markdown markers.
    let wrap_text = |marker: &'static str| {
        let cur = text.get();
        set_text.set(format!("{marker}{cur}{marker}"));
    };

    view! {
        <div class="input-bar">
            // ── Destination label ─────────────────────────────────────────
            <div class="input-dest-row">
                <span class="input-dest-label">
                    {move || {
                        let ch = ctx.active_channel.get();
                        if ctx.is_dm_view() {
                            format!("DM → {}", ctx.dm_peer())
                        } else {
                            format!("#{}", ch.trim_start_matches('#'))
                        }
                    }}
                </span>
                // Optional topic field (channel messages only)
                {move || {
                    if ctx.is_dm_view() { return view! { <></> }.into_view(); }
                    view! {
                        <input
                            class="topic-input"
                            type="text"
                            placeholder="topic (optional)"
                            prop:value=topic
                            on:input=move |e| set_topic.set(event_target_value(&e))
                        />
                    }.into_view()
                }}
            </div>

            // ── Formatting toolbar ────────────────────────────────────────
            <div class="format-toolbar">
                <button class="format-btn" title="Bold" on:click=move |_| {
                    set_text.update(|t| *t = format!("**{t}**"));
                }><b>"B"</b></button>
                <button class="format-btn" title="Italic" on:click=move |_| {
                    set_text.update(|t| *t = format!("_{t}_"));
                }><i>"I"</i></button>
                <button class="format-btn format-btn-mono" title="Inline code" on:click=move |_| {
                    set_text.update(|t| *t = format!("`{t}`"));
                }>"`·`"</button>
                <button class="format-btn format-btn-mono" title="Code block" on:click=move |_| {
                    set_text.update(|t| *t = format!("```\n{t}\n```"));
                }>"</>",</button>
                <button class="format-btn" title="Blockquote" on:click=move |_| {
                    set_text.update(|t| *t = format!("> {t}"));
                }>"❝"</button>
                <div class="format-spacer" />
                <span class="format-hint">"Shift+Enter = newline · Enter = send"</span>
            </div>

            // ── Compose row ───────────────────────────────────────────────
            <div class="input-row">
                <textarea
                    class="input-text"
                    placeholder=move || {
                        let ch = ctx.active_channel.get();
                        if ctx.is_dm_view() {
                            format!("Message {}", ctx.dm_peer())
                        } else {
                            format!("Message #{}", ch.trim_start_matches('#'))
                        }
                    }
                    prop:value=text
                    attr:disabled=move || if sending.get() { Some("disabled") } else { None }
                    on:input=move |e| set_text.set(event_target_value(&e))
                    on:keydown=move |e| {
                        if e.key() == "Enter" && !e.shift_key() {
                            e.prevent_default();
                            do_send();
                        }
                    }
                />
                <button
                    class="send-btn"
                    attr:disabled=move || if sending.get() { Some("disabled") } else { None }
                    on:click=move |_| do_send()
                >
                    {move || if sending.get() { "…" } else { "Send" }}
                </button>
            </div>
        </div>
    }
}
