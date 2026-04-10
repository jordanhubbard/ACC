mod types;
mod components;
mod markdown;

use leptos::*;
use types::*;
use components::{
    login::LoginScreen,
    sidebar::Sidebar,
    message_pane::MessagePane,
    thread_pane::ThreadPane,
    input_bar::InputBar,
    command_palette::CommandPalette,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use std::collections::HashMap;

fn ls_get(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(key).ok().flatten())
        .filter(|v| !v.is_empty())
}

fn ls_set(key: &str, val: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(key, val);
    }
}

const LS_KEY_TOKEN: &str = "ccc_token";
const LS_KEY_USER:  &str = "ccc_username";

// ── Root app — handles auth gate ──────────────────────────────────────────────

#[component]
fn App() -> impl IntoView {
    let (token, set_token) = create_signal(ls_get(LS_KEY_TOKEN));
    let (username, set_username) = create_signal(
        ls_get(LS_KEY_USER).unwrap_or_default(),
    );

    let on_login = move |(tok, user): (String, String)| {
        ls_set(LS_KEY_TOKEN, &tok);
        ls_set(LS_KEY_USER, &user);
        set_token.set(Some(tok));
        set_username.set(user);
    };

    view! {
        {move || match token.get() {
            None => view! { <LoginScreen on_login=on_login.clone() /> }.into_view(),
            Some(_) => view! {
                <ChatApp token=token set_token=set_token username=username />
            }.into_view(),
        }}
    }
}

// ── Main chat app — sets up context and global handlers ───────────────────────

#[component]
fn ChatApp(
    token: ReadSignal<Option<String>>,
    set_token: WriteSignal<Option<String>>,
    username: ReadSignal<String>,
) -> impl IntoView {
    let (messages, set_messages)  = create_signal(Vec::<BusMessage>::new());
    let (presence, set_presence)  = create_signal(Vec::<PresenceEntry>::new());
    let (connected, set_connected) = create_signal(false);

    let active_channel = create_rw_signal("#general".to_string());
    let open_thread    = create_rw_signal(Option::<String>::None);
    let palette_open   = create_rw_signal(false);
    let emoji_target   = create_rw_signal(Option::<String>::None);
    let read_counts    = create_rw_signal(HashMap::<String, usize>::new());

    // ── Load history ──────────────────────────────────────────────────────────
    {
        let tok      = token.get_untracked().unwrap_or_default();
        let set_msgs = set_messages;
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/bus/messages?limit=500")
                .header("Authorization", &format!("Bearer {tok}"))
                .send()
                .await
            {
                if let Ok(msgs) = resp.json::<Vec<BusMessage>>().await {
                    set_msgs.set(msgs);
                }
            }
        });
    }

    // ── Load presence ─────────────────────────────────────────────────────────
    {
        let tok   = token.get_untracked().unwrap_or_default();
        let set_p = set_presence;
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/bus/presence")
                .header("Authorization", &format!("Bearer {tok}"))
                .send()
                .await
            {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    if let Some(obj) = val.as_object() {
                        let entries = obj.iter().map(|(name, info)| {
                            let status = info.get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("offline");
                            PresenceEntry { agent: name.clone(), online: status == "online" }
                        }).collect();
                        set_p.set(entries);
                    }
                }
            }
        });
    }

    // ── SSE stream ────────────────────────────────────────────────────────────
    {
        let tok      = token.get_untracked().unwrap_or_default();
        let set_msgs = set_messages;
        let set_conn = set_connected;

        let stream_url = if tok.is_empty() {
            "/bus/stream".to_string()
        } else {
            format!("/bus/stream?token={tok}")
        };

        if let Ok(es) = web_sys::EventSource::new(&stream_url) {
            let es_cleanup = es.clone();

            let open_cb = Closure::<dyn FnMut()>::new(move || {
                set_conn.set(true);
            });
            es.set_onopen(Some(open_cb.as_ref().unchecked_ref()));
            open_cb.forget();

            let msg_cb = Closure::<dyn FnMut(_)>::new(move |e: web_sys::MessageEvent| {
                let data = e.data().as_string().unwrap_or_default();
                if data.starts_with(':') || data.is_empty() { return; }
                if let Ok(msg) = serde_json::from_str::<BusMessage>(&data) {
                    // Accept text messages (including replies) and reactions
                    if msg.is_text() || msg.is_reaction() {
                        set_msgs.update(|v| {
                            // Dedup by stable_id
                            let id = msg.stable_id();
                            if !id.is_empty() && v.iter().any(|m| m.stable_id() == id) {
                                return;
                            }
                            v.push(msg);
                        });
                    }
                }
            });
            es.set_onmessage(Some(msg_cb.as_ref().unchecked_ref()));
            msg_cb.forget();

            let err_cb = Closure::<dyn FnMut(_)>::new(move |_: web_sys::ErrorEvent| {
                set_conn.set(false);
            });
            es.set_onerror(Some(err_cb.as_ref().unchecked_ref()));
            err_cb.forget();

            on_cleanup(move || es_cleanup.close());
        }
    }

    // ── Global Cmd+K keyboard handler ────────────────────────────────────────
    {
        let kb_cb = Closure::<dyn FnMut(_)>::new(move |e: web_sys::KeyboardEvent| {
            if (e.meta_key() || e.ctrl_key()) && e.key() == "k" {
                e.prevent_default();
                palette_open.update(|v| *v = !*v);
            }
            if e.key() == "Escape" {
                palette_open.set(false);
                emoji_target.set(None);
            }
        });
        if let Some(win) = web_sys::window() {
            win.add_event_listener_with_callback("keydown", kb_cb.as_ref().unchecked_ref()).ok();
        }
        kb_cb.forget();
    }

    let ctx = ChatContext {
        token, set_token, username,
        messages, set_messages,
        active_channel, open_thread,
        palette_open, emoji_target,
        presence, connected,
        read_counts,
    };
    provide_context(ctx);

    view! {
        <div class="clawchat-app">
            <Sidebar />
            <div class="chat-main">
                <div class="chat-body">
                    <MessagePane />
                    <InputBar />
                </div>
                <ThreadPane />
            </div>
            <CommandPalette />
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App /> });
}
