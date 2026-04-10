/// Thread side-pane — shows parent message + replies.
use leptos::*;
use leptos::html::Div;
use wasm_bindgen_futures::spawn_local;
use crate::types::{BusMessage, ChatContext};
use crate::components::message::{Message, MessageList, fmt_time};

#[component]
pub fn ThreadPane() -> impl IntoView {
    let ctx         = use_context::<ChatContext>().expect("ChatContext");
    let (text, set_text) = create_signal(String::new());
    let (sending, set_sending) = create_signal(false);
    let replies_ref  = create_node_ref::<Div>();

    // Auto-scroll replies to bottom when new replies arrive
    {
        let open_thread = ctx.open_thread;
        create_effect(move |_| {
            let _ = ctx.messages.get();
            let _ = open_thread.get();
            if let Some(el) = replies_ref.get() {
                el.set_scroll_top(el.scroll_height());
            }
        });
    }

    let do_send = move || {
        let body = text.get();
        if body.trim().is_empty() || sending.get() { return; }
        let Some(parent_id) = ctx.open_thread.get() else { return };

        let tok     = ctx.token.get().unwrap_or_default();
        let from    = ctx.username.get();
        let subject = {
            // Find the parent message to get its subject
            ctx.messages.get()
                .iter()
                .find(|m| m.stable_id() == parent_id)
                .and_then(|m| m.subject.clone())
                .unwrap_or_else(|| ctx.active_channel.get())
        };

        set_text.set(String::new());
        set_sending.set(true);

        let set_msgs = ctx.set_messages;
        spawn_local(async move {
            let payload = serde_json::json!({
                "from": from,
                "to": "all",
                "type": "text",
                "subject": subject,
                "thread_id": parent_id,
                "body": body,
                "mime": "text/plain",
            });
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

    view! {
        {move || {
            let Some(parent_id) = ctx.open_thread.get() else {
                return view! { <></> }.into_view();
            };

            let parent_msg = ctx.messages.get()
                .into_iter()
                .find(|m| m.stable_id() == parent_id);

            let replies    = ctx.thread_replies(&parent_id);
            let channel    = parent_msg.as_ref()
                .and_then(|m| m.subject.clone())
                .unwrap_or_else(|| ctx.active_channel.get());
            let channel_label = channel.trim_start_matches('#').to_string();

            view! {
                <div class="thread-pane">
                    <div class="thread-header">
                        <span class="thread-title">"Thread"</span>
                        <span class="thread-channel">"# "{channel_label}</span>
                        <button class="thread-close-btn" on:click=move |_| {
                            ctx.open_thread.set(None);
                        }>"✕"</button>
                    </div>

                    <div class="thread-parent">
                        {if let Some(parent) = parent_msg {
                            view! {
                                <div class="msg-group">
                                    <Message msg=parent show_header=true in_thread=true />
                                </div>
                            }.into_view()
                        } else {
                            view! { <div class="thread-parent-missing">"(message not found)"</div> }.into_view()
                        }}
                    </div>

                    <div class="thread-divider">
                        <span class="thread-reply-count">
                            {replies.len()}" "{if replies.len() == 1 {"reply"} else {"replies"}}
                        </span>
                        <span class="thread-divider-line" />
                    </div>

                    <div class="thread-replies" node_ref=replies_ref>
                        {if replies.is_empty() {
                            view! {
                                <div class="thread-empty">"No replies yet. Be the first!"</div>
                            }.into_view()
                        } else {
                            view! { <MessageList messages=replies in_thread=true /> }.into_view()
                        }}
                    </div>

                    <div class="thread-input-area">
                        <textarea
                            class="thread-input"
                            placeholder="Reply in thread…"
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
                            {move || if sending.get() { "…" } else { "↩" }}
                        </button>
                    </div>
                </div>
            }.into_view()
        }}
    }
}
