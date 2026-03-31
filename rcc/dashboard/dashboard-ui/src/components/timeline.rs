//! agentOS Agent Lifecycle Timeline
//!
//! Fetches `/api/agentos/events` and renders a horizontal SVG timeline with
//! one row per agent slot and coloured markers for each event type.
//! Auto-refreshes every 10 seconds.

use leptos::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentEvent {
    /// Unix timestamp (ms)
    pub ts: f64,
    /// Slot index (0-7)
    pub slot_id: Option<u32>,
    /// Event type string: spawn, exit, cap_grant, cap_revoke,
    ///   quota_exceeded, fault, watchdog_reset, memory_alert
    #[serde(rename = "type")]
    pub event_type: String,
    /// Optional detail string
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentEventsResponse {
    pub events: Vec<AgentEvent>,
    pub slots: Option<Vec<u32>>,
    pub generated_at: Option<f64>,
}

// ── Fetch helper ──────────────────────────────────────────────────────────────

async fn fetch_events() -> AgentEventsResponse {
    let Ok(resp) = gloo_net::http::Request::get("/api/agentos/events")
        .send()
        .await
    else {
        return AgentEventsResponse::default();
    };
    resp.json::<AgentEventsResponse>()
        .await
        .unwrap_or_default()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return the CSS hex colour for each event type.
fn event_color(event_type: &str) -> &'static str {
    match event_type {
        "spawn"          => "#3fb950",
        "exit"           => "#2ea043",
        "cap_grant"      => "#58a6ff",
        "cap_revoke"     => "#f85149",
        "quota_exceeded" => "#e3b341",
        "fault"          => "#f85149",
        "watchdog_reset" => "#d29922",
        "memory_alert"   => "#a371f7",
        _                => "#8b949e",
    }
}

/// Format a Unix-ms timestamp as HH:MM.
fn fmt_hhmm(ts_ms: f64) -> String {
    let secs = (ts_ms / 1000.0) as u64;
    let mins = (secs / 60) % 60;
    let hours = (secs / 3600) % 24;
    format!("{hours:02}:{mins:02}")
}

/// Format a Unix-ms timestamp as HH:MM:SS.
fn fmt_hhmmss(ts_ms: f64) -> String {
    let secs = (ts_ms / 1000.0) as u64;
    let ss = secs % 60;
    let mm = (secs / 60) % 60;
    let hh = (secs / 3600) % 24;
    format!("{hh:02}:{mm:02}:{ss:02}")
}

// ── Component ─────────────────────────────────────────────────────────────────

#[component]
pub fn Timeline() -> impl IntoView {
    // Current data
    let (data, set_data)     = create_signal(AgentEventsResponse::default());
    let (loading, set_loading) = create_signal(true);
    let (last_update, set_last_update) = create_signal(String::new());

    // Hovered event details for the tooltip (slot_id, event_type, ts_ms, details)
    let (tooltip, set_tooltip) = create_signal(Option::<(u32, String, f64, Option<String>)>::None);

    // Load function
    let load = {
        let set_data     = set_data;
        let set_loading  = set_loading;
        let set_last_update = set_last_update;
        move || {
            let set_data     = set_data;
            let set_loading  = set_loading;
            let set_last_update = set_last_update;
            spawn_local(async move {
                let result = fetch_events().await;
                set_data.set(result);
                set_loading.set(false);
                // Record update time
                let ts = js_sys::Date::now();
                set_last_update.set(fmt_hhmmss(ts));
            });
        }
    };

    // Initial load
    {
        let load = load.clone();
        create_effect(move |_| { load(); });
    }

    // Auto-refresh every 10 seconds
    {
        let load = load.clone();
        let interval = gloo_timers::callback::Interval::new(10_000, move || load());
        on_cleanup(move || drop(interval));
    }

    // Time window: last 30 minutes
    const WINDOW_MS: f64 = 30.0 * 60.0 * 1000.0;

    view! {
        <div class="tl-container">
            <div class="tl-header">
                <h2 class="tl-title">"⏱️ Agent Lifecycle Timeline"</h2>
                <div class="tl-header-right">
                    {move || {
                        let u = last_update.get();
                        if u.is_empty() { view! { <span></span> }.into_view() }
                        else { view! { <span class="tl-updated">"updated " {u}</span> }.into_view() }
                    }}
                    <button
                        class="tl-refresh-btn"
                        on:click={
                            let load = load.clone();
                            move |_| load()
                        }
                    >"↻ Refresh"</button>
                </div>
            </div>

            <p class="tl-subtitle">"agentOS per-slot events · last 30 min · auto-refreshes every 10s"</p>

            // Legend
            <div class="tl-legend">
                {["spawn", "exit", "cap_grant", "cap_revoke", "quota_exceeded",
                  "fault", "watchdog_reset", "memory_alert"]
                    .iter()
                    .map(|et| {
                        let color = event_color(et);
                        let is_fault = *et == "fault";
                        view! {
                            <div class="tl-legend-item">
                                <span
                                    class="tl-legend-dot"
                                    style={format!(
                                        "background:{color};{}",
                                        if is_fault { "border-radius:2px;transform:rotate(45deg);" } else { "" }
                                    )}
                                ></span>
                                <span>{*et}</span>
                            </div>
                        }
                    })
                    .collect_view()
                }
            </div>

            {move || {
                if loading.get() {
                    return view! { <p class="tl-spinner">"Loading…"</p> }.into_view();
                }

                let d = data.get();
                let now_ms = js_sys::Date::now();
                let t_min = now_ms - WINDOW_MS;

                // Time axis labels (6 evenly spaced)
                let labels: Vec<String> = (0..=5)
                    .map(|i| fmt_hhmm(t_min + WINDOW_MS * (i as f64) / 5.0))
                    .collect();

                // Group events by slot
                let mut slots: [Vec<AgentEvent>; 8] = Default::default();
                for ev in &d.events {
                    let slot = ev.slot_id.unwrap_or(0) as usize;
                    if slot < 8 {
                        slots[slot].push(ev.clone());
                    }
                }

                view! {
                    <div class="tl-panel">
                        // Slot rows
                        {(0usize..8).map(|s| {
                            let slot_events = slots[s].clone();
                            view! {
                                <div class="tl-row">
                                    <div class="tl-slot-label">
                                        {format!("Slot {s}")}
                                    </div>
                                    <div class="tl-axis">
                                        {slot_events.iter().map(|ev| {
                                            let pct = ((ev.ts - t_min) / WINDOW_MS * 100.0)
                                                .max(0.0).min(99.0);
                                            let color = event_color(&ev.event_type);
                                            let is_fault = ev.event_type == "fault";
                                            let ev_clone = ev.clone();
                                            let slot_id = s as u32;
                                            view! {
                                                <span
                                                    class={format!(
                                                        "tl-marker{}",
                                                        if is_fault { " tl-marker-fault" } else { "" }
                                                    )}
                                                    style={format!(
                                                        "left:{pct:.2}%;background:{color};"
                                                    )}
                                                    on:mouseenter={
                                                        let ev2 = ev_clone.clone();
                                                        move |_| set_tooltip.set(Some((
                                                            slot_id,
                                                            ev2.event_type.clone(),
                                                            ev2.ts,
                                                            ev2.details.clone(),
                                                        )))
                                                    }
                                                    on:mouseleave=move |_| set_tooltip.set(None)
                                                ></span>
                                            }
                                        }).collect_view()}
                                    </div>
                                </div>
                            }
                        }).collect_view()}

                        // Time axis
                        <div class="tl-time-axis">
                            {labels.iter().map(|l| {
                                view! { <span>{l.clone()}</span> }
                            }).collect_view()}
                        </div>
                    </div>

                    // Tooltip (fixed, shows on hover)
                    {move || {
                        match tooltip.get() {
                            None => view! { <div class="tl-tooltip tl-tooltip-hidden"></div> }.into_view(),
                            Some((slot, etype, ts, details)) => {
                                let time_str = fmt_hhmmss(ts);
                                view! {
                                    <div class="tl-tooltip tl-tooltip-visible">
                                        <div class="tl-tt-type">{etype.clone()}</div>
                                        <div class="tl-tt-meta">
                                            {format!("Slot {slot} · {time_str}")}
                                        </div>
                                        {details.map(|d| view! {
                                            <div class="tl-tt-detail">{d}</div>
                                        })}
                                    </div>
                                }.into_view()
                            }
                        }
                    }}
                }.into_view()
            }}
        </div>
    }
}
