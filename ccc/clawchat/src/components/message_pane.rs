/// Main message pane — channel view or DM view.
use leptos::*;
use leptos::html::Div;
use crate::types::ChatContext;
use crate::components::message::MessageList;

#[component]
pub fn MessagePane() -> impl IntoView {
    let ctx      = use_context::<ChatContext>().expect("ChatContext");
    let list_ref = create_node_ref::<Div>();

    // Auto-scroll to bottom when messages or active channel change
    create_effect(move |_| {
        let _ = ctx.messages.get();
        let _ = ctx.active_channel.get();
        if let Some(el) = list_ref.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });

    // Close emoji picker on click anywhere in the pane
    let close_picker = move |_| {
        if ctx.emoji_target.get().is_some() {
            ctx.emoji_target.set(None);
        }
    };

    view! {
        <div class="message-pane" on:click=close_picker>
            // Channel / DM header
            <div class="channel-header">
                {move || {
                    let ch = ctx.active_channel.get();
                    if ctx.is_dm_view() {
                        let peer = ctx.dm_peer();
                        let online = ctx.presence.get()
                            .iter()
                            .find(|p| p.agent == peer)
                            .map(|p| p.online)
                            .unwrap_or(false);
                        view! {
                            <span class="channel-header-at">"@"</span>
                            <span class="channel-header-name">{peer.clone()}</span>
                            <span class=if online { "presence-pill online" } else { "presence-pill offline" }>
                                {if online { "online" } else { "offline" }}
                            </span>
                        }.into_view()
                    } else {
                        view! {
                            <span class="channel-header-hash">"#"</span>
                            <span class="channel-header-name">
                                {ch.trim_start_matches('#').to_string()}
                            </span>
                            <span class="channel-header-count">
                                {move || {
                                    let c = ctx.active_channel.get();
                                    let n = ctx.channel_messages(&c).len();
                                    format!("{n} message{}", if n == 1 { "" } else { "s" })
                                }}
                            </span>
                        }.into_view()
                    }
                }}
            </div>

            // Message list
            <div class="message-list" node_ref=list_ref>
                {move || {
                    let ch   = ctx.active_channel.get();
                    let msgs = if ctx.is_dm_view() {
                        let me   = ctx.username.get();
                        let peer = ctx.dm_peer();
                        ctx.dm_messages(&me, &peer)
                    } else {
                        ctx.channel_messages(&ch)
                    };

                    if msgs.is_empty() {
                        return view! {
                            <div class="messages-empty">
                                <span class="messages-empty-icon">"💬"</span>
                                <p>"No messages yet — say something!"</p>
                            </div>
                        }.into_view();
                    }

                    view! { <MessageList messages=msgs /> }.into_view()
                }}
            </div>
        </div>
    }
}
