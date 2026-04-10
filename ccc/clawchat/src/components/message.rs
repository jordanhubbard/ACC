/// Individual message component with markdown, reactions, hover actions, thread reply badge.
use leptos::*;
use wasm_bindgen_futures::spawn_local;
use crate::types::{BusMessage, ChatContext, QUICK_REACTIONS};
use crate::markdown::render_markdown;

/// Format ISO-8601 timestamp → "HH:MM"
pub fn fmt_time(ts: &str) -> String {
    ts.split('T')
        .nth(1)
        .and_then(|t| t.split('.').next())
        .and_then(|t| t.get(..5))
        .unwrap_or(ts)
        .to_string()
}

/// Format ISO-8601 timestamp → "Mon DD" or "Today"
pub fn fmt_date_header(ts: &str) -> String {
    ts.split('T').next().unwrap_or(ts).to_string()
}

/// Avatar initials + deterministic color class (a-z picks one of 8 colors).
pub fn avatar_class(name: &str) -> &'static str {
    let idx = name.chars().next().map(|c| c as usize % 8).unwrap_or(0);
    match idx {
        0 => "avatar-a", 1 => "avatar-b", 2 => "avatar-c", 3 => "avatar-d",
        4 => "avatar-e", 5 => "avatar-f", 6 => "avatar-g", _ => "avatar-h",
    }
}

pub fn avatar_initial(name: &str) -> String {
    name.chars().next().unwrap_or('?').to_uppercase().to_string()
}

/// Single message row. `show_header` = true for the first in an author group.
#[component]
pub fn Message(
    msg: BusMessage,
    show_header: bool,
    /// True when rendered inside the thread pane (suppresses reply button).
    #[prop(default = false)]
    in_thread: bool,
) -> impl IntoView {
    let ctx = use_context::<ChatContext>().expect("ChatContext");

    let msg_id    = msg.stable_id();
    let author    = msg.display_from();
    let ts        = msg.ts.clone().unwrap_or_default();
    let body_text = msg.body.clone().unwrap_or_default();
    let msg_subject = msg.subject.clone().unwrap_or_default();

    // Render markdown to HTML
    let body_html = render_markdown(&body_text);

    // Emoji picker visible for THIS message
    let picker_msg_id  = msg_id.clone();
    let picker_visible = create_memo(move |_| {
        ctx.emoji_target.get().as_deref() == Some(picker_msg_id.as_str())
    });

    let id_for_open_thread = msg_id.clone();
    let id_for_reaction    = msg_id.clone();
    let id_for_picker      = msg_id.clone();

    view! {
        <div class="msg-row">
            // Header (avatar + author + time) — only first in group
            {if show_header {
                let av_class = avatar_class(&author);
                let av_init  = avatar_initial(&author);
                let a        = author.clone();
                let t        = fmt_time(&ts);
                view! {
                    <div class="msg-header">
                        <span class={format!("msg-avatar {av_class}")}>{av_init}</span>
                        <span class="msg-author">{a}</span>
                        <span class="msg-time">{t}</span>
                    </div>
                }.into_view()
            } else {
                view! {
                    <span class="msg-continuation-time">{fmt_time(&ts)}</span>
                }.into_view()
            }}

            // Message body (markdown rendered)
            <div class="msg-body" inner_html=body_html />

            // Reactions row
            {move || {
                let me      = ctx.username.get();
                let tok     = ctx.token.get().unwrap_or_default();
                let subj    = msg_subject.clone();
                let mid     = id_for_reaction.clone();
                let groups  = ctx.reactions_for(&mid, &me);
                if groups.is_empty() {
                    return view! { <></> }.into_view();
                }
                view! {
                    <div class="msg-reactions">
                        {groups.into_iter().map(|g| {
                            let e      = g.emoji.clone();
                            let e_send = e.clone();
                            let mid2   = mid.clone();
                            let subj2  = subj.clone();
                            let tok2   = tok.clone();
                            let from2  = me.clone();
                            let mine   = g.reacted_by_me;
                            let cnt    = g.count;
                            view! {
                                <button
                                    class=move || if mine { "reaction-btn reaction-mine" } else { "reaction-btn" }
                                    title=e.clone()
                                    on:click=move |_| {
                                        let action = if mine { "remove" } else { "add" };
                                        let payload = serde_json::json!({
                                            "type": "reaction",
                                            "from": from2,
                                            "subject": subj2,
                                            "target": mid2,
                                            "emoji": e_send,
                                            "action": action,
                                        });
                                        let tok = tok2.clone();
                                        spawn_local(async move {
                                            let _ = gloo_net::http::Request::post("/bus/send")
                                                .header("Authorization", &format!("Bearer {tok}"))
                                                .header("Content-Type", "application/json")
                                                .body(payload.to_string())
                                                .unwrap()
                                                .send()
                                                .await;
                                        });
                                    }
                                >
                                    {e.clone()}" "{cnt}
                                </button>
                            }
                        }).collect::<Vec<_>>().into_view()}
                    </div>
                }.into_view()
            }}

            // Reply count badge (not shown in thread pane)
            {if !in_thread {
                let mid3 = msg_id.clone();
                view! {
                    {move || {
                        let count = ctx.reply_count(&mid3);
                        if count == 0 {
                            return view! { <></> }.into_view();
                        }
                        let mid4 = mid3.clone();
                        // Find latest replier
                        let replies = ctx.thread_replies(&mid3);
                        let last_from = replies.last().and_then(|r| r.from.clone()).unwrap_or_default();
                        view! {
                            <button class="reply-count-badge" on:click=move |_| {
                                ctx.open_thread.set(Some(mid4.clone()));
                            }>
                                <span class={format!("mini-avatar {}", avatar_class(&last_from))}>
                                    {avatar_initial(&last_from)}
                                </span>
                                {format!("{count} repl{}", if count == 1 {"y"} else {"ies"})}
                            </button>
                        }.into_view()
                    }}
                }.into_view()
            } else {
                view! { <></> }.into_view()
            }}

            // Hover action bar
            <div class="msg-hover-bar">
                // Emoji react button
                <button class="msg-action-btn" title="Add reaction" on:click=move |_| {
                    let cur = ctx.emoji_target.get();
                    if cur.as_deref() == Some(&id_for_picker) {
                        ctx.emoji_target.set(None);
                    } else {
                        ctx.emoji_target.set(Some(id_for_picker.clone()));
                    }
                }>"😊"</button>

                // Reply in thread (not shown inside thread pane)
                {if !in_thread {
                    let mid5 = id_for_open_thread.clone();
                    view! {
                        <button class="msg-action-btn" title="Reply in thread" on:click=move |_| {
                            ctx.open_thread.set(Some(mid5.clone()));
                        }>"💬"</button>
                    }.into_view()
                } else {
                    view! { <></> }.into_view()
                }}
            </div>

            // Emoji picker (positioned absolutely, shown when this message is the target)
            {move || {
                if !picker_visible.get() { return view! { <></> }.into_view(); }
                let subj  = msg.subject.clone().unwrap_or_default();
                let mid   = msg.stable_id();
                let me    = ctx.username.get();
                let tok   = ctx.token.get().unwrap_or_default();
                view! {
                    <div class="emoji-picker">
                        {QUICK_REACTIONS.iter().map(|&e| {
                            let emoji  = e.to_string();
                            let e_send = emoji.clone();
                            let mid2   = mid.clone();
                            let subj2  = subj.clone();
                            let from2  = me.clone();
                            let tok2   = tok.clone();
                            view! {
                                <button class="emoji-option" on:click=move |_| {
                                    ctx.emoji_target.set(None);
                                    let payload = serde_json::json!({
                                        "type": "reaction",
                                        "from": from2,
                                        "subject": subj2,
                                        "target": mid2,
                                        "emoji": e_send,
                                        "action": "add",
                                    });
                                    let tok = tok2.clone();
                                    spawn_local(async move {
                                        let _ = gloo_net::http::Request::post("/bus/send")
                                            .header("Authorization", &format!("Bearer {tok}"))
                                            .header("Content-Type", "application/json")
                                            .body(payload.to_string())
                                            .unwrap()
                                            .send()
                                            .await;
                                    });
                                }>{emoji}</button>
                            }
                        }).collect::<Vec<_>>().into_view()}
                    </div>
                }.into_view()
            }}
        </div>
    }
}

/// Render a list of messages with date dividers and author grouping.
/// Used by both message_pane and thread_pane.
#[component]
pub fn MessageList(
    messages: Vec<BusMessage>,
    #[prop(default = false)]
    in_thread: bool,
) -> impl IntoView {
    let mut rendered: Vec<leptos::View> = Vec::new();
    let mut last_date   = String::new();
    let mut last_author = String::new();

    for msg in messages {
        let ts   = msg.ts.clone().unwrap_or_default();
        let date = fmt_date_header(&ts);
        let auth = msg.display_from();

        // Date divider
        if date != last_date && !date.is_empty() {
            let d = date.clone();
            rendered.push(view! {
                <div class="date-divider">
                    <span class="date-divider-line" />
                    <span class="date-divider-label">{d}</span>
                    <span class="date-divider-line" />
                </div>
            }.into_view());
            last_date   = date;
            last_author = String::new();
        }

        // Topic label (Zulip-style)
        if let Some(topic) = &msg.topic {
            if !topic.is_empty() {
                let t = topic.clone();
                rendered.push(view! {
                    <div class="topic-label">
                        <span class="topic-label-icon">"⟶"</span>
                        <span class="topic-label-text">{t}</span>
                    </div>
                }.into_view());
            }
        }

        let show_header = auth != last_author;
        last_author = auth;

        let class = if show_header { "msg-group" } else { "msg-continuation" };
        let m = msg.clone();
        rendered.push(view! {
            <div class=class>
                <Message msg=m show_header=show_header in_thread=in_thread />
            </div>
        }.into_view());
    }

    rendered.into_view()
}
