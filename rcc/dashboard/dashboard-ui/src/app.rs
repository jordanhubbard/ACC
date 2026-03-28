use leptos::*;

use crate::components::{
    agent_cards::AgentCards,
    idea_incubator::IdeaIncubator,
    metrics::Metrics,
    squirrelbus::SquirrelBus,
    work_queue::WorkQueue,
};

#[component]
pub fn App() -> impl IntoView {
    view! {
        <div class="dashboard">
            <header class="dash-header">
                <div class="dash-logo">
                    <span class="logo-icon">"⚡"</span>
                    <span class="logo-text">"Rocky Command Center"</span>
                </div>
                <div class="dash-subtitle">"v2 — Rust/WASM"</div>
            </header>
            <main class="dash-main">
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
            </main>
        </div>
    }
}
