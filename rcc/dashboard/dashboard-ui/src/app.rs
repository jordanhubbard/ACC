use leptos::*;

use crate::components::{
    agent_cards::AgentCards,
    geek_view::GeekView,
    idea_incubator::IdeaIncubator,
    metrics::Metrics,
    squirrelbus::SquirrelBus,
    work_queue::WorkQueue,
};

#[component]
pub fn App() -> impl IntoView {
    let (show_geek, set_show_geek) = create_signal(false);

    view! {
        <div class="dashboard">
            <header class="dash-header">
                <div class="dash-logo">
                    <span class="logo-icon">"⚡"</span>
                    <span class="logo-text">"Rocky Command Center"</span>
                </div>
                <div class="dash-subtitle">"v2 — Rust/WASM"</div>
                <div class="dash-tabs">
                    <button
                        class="tab-btn"
                        class:tab-active=move || !show_geek.get()
                        on:click=move |_| set_show_geek.set(false)
                    >"Dashboard"</button>
                    <button
                        class="tab-btn"
                        class:tab-active=move || show_geek.get()
                        on:click=move |_| set_show_geek.set(true)
                    >"Geek View"</button>
                </div>
            </header>
            <main class="dash-main">
                {move || if show_geek.get() {
                    view! { <GeekView /> }.into_view()
                } else {
                    view! {
                        <div class="dash-main-content">
                            <div class="dash-row dash-row-top">
                                <AgentCards />
                                <Metrics />
                            </div>
                            <div class="dash-row">
                                <WorkQueue />
                            </div>
                            <div class="dash-row">
                                <SquirrelBus />
                                <IdeaIncubator />
                            </div>
                        </div>
                    }.into_view()
                }}
            </main>
        </div>
    }
}
