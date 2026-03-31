//! agentOS Agent Lifecycle Timeline
//!
//! Fetches `/api/agentos/timeline` and renders:
//!   1. A horizontal SVG timeline with one row per agent slot and coloured markers.
//!   2. A vertical event list (most recent first) with coloured dot, timestamp,
//!      slot badge, and detail text.
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
    /// Event type: spawn, cap_grant, cap_revoke, quota_exceeded, fault,
    ///   watchdog_reset, memory_alert, hotreload
    pub event_type: String,
    /// Human-readable detail string
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentEventsResponse {
    pub events: Vec<AgentEvent>,
    pub generated_at: Option<f64>,
}

// ── Fetch helper ──────────────────────────────────────────────────────────────

async fn fetch_events() -> AgentEventsResponse {
    let Ok(resp) = gloo_net::http::Request::get("/api/agentos/timeline")
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

/// CSS hex colour for each event type.
/// Colors: green=spawn, blue=cap, orange=quota/fault, red=watchdog_reset.
fn event_color(event_type: &str) -> &'static str {
    match event_type {
        "spawn"          => "#3fb950",  // green
        "hotreload"      => "#2ea043",  // teal
        "cap_grant"      => "#58a6ff",  // blue
        "cap_revoke"     => "#58a6ff",  // blue
        "quota_exceeded" => "#f0883e",  // orange
        "fault"          => "#f0883e",  // orange
        "watchdog_reset" => "#f85149",  // red
        "memory_alert"   => "#a371f7",  // purple
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
    let (data, set_data)         = create_signal(AgentEventsResponse::default());
    let (loading, set_loading)   = create_signal(true);
    let (last_update, set_last_update) = create_signal(String::new());

    // Hovered event: (slot_id, event_type, ts_ms, detail)
    let (tooltip, set_tooltip) = create_signal(Option::<(u32, String, f64, Option<String>)>::None);

    let load = {
        let set_data        = set_data;
        let set_loading     = set_loading;
        let set_last_update = set_last_update;
        move || {
            let set_data        = set_data;
            let set_loading     = set_loading;
            let set_last_update = set_last_update;
            spawn_local(async move {
                let result = fetch_events().await;
                set_data.set(result);
                set_loading.set(false);
                set_last_update.set(fmt_hhmmss(js_sys::Date::now()));
            });
        }
    };

    // Initial load
    { let load = load.clone(); create_effect(move |_| { load(); }); }

    // Auto-refresh every 10 seconds
    {
        let load = load.clone();
        let interval = gloo_timers::callback::Interval::new(10_000, move || load());
        on_cleanup(move || drop(interval));
    }

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
                        on:click={ let load = load.clone(); move |_| load() }
                    >"↻ Refresh"</button>
                </div>
            </div>

            <p class="tl-subtitle">"agentOS per-slot events · last 30 min · auto-refreshes every 10s"</p>

            // Legend
            <div class="tl-legend">
                {["spawn", "hotreload", "cap_grant", "cap_revoke", "quota_exceeded",
                  "fault", "watchdog_reset", "memory_alert"]
                    .iter()
                    .map(|et| {
                        let color = event_color(et);
                        view! {
                            <div class="tl-legend-item">
                                <span
                                    class="tl-legend-dot"
                                    style={format!("background:{color};")}
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

                // Group events by slot for the SVG panel
                // Events from the API are sorted desc (newest first); re-sort asc for the SVG axis.
                let mut asc_events = d.events.clone();
                asc_events.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap_or(std::cmp::Ordering::Equal));

                let mut slots: [Vec<AgentEvent>; 8] = Default::default();
                for ev in &asc_events {
                    let slot = ev.slot_id.unwrap_or(0) as usize;
                    if slot < 8 { slots[slot].push(ev.clone()); }
                }

                view! {
                    // ── Horizontal per-slot timeline ─────────────────────────
                    <div class="tl-panel">
                        {(0usize..8).map(|s| {
                            let slot_events = slots[s].clone();
                            view! {
                                <div class="tl-row">
                                    <div class="tl-slot-label">{format!("Slot {s}")}</div>
                                    <div class="tl-axis">
                                        {slot_events.iter().map(|ev| {
                                            let pct = ((ev.ts - t_min) / WINDOW_MS * 100.0)
                                                .max(0.0).min(99.0);
                                            let color = event_color(&ev.event_type);
                                            let ev2 = ev.clone();
                                            let slot_id = s as u32;
                                            view! {
                                                <span
                                                    class="tl-marker"
                                                    style={format!("left:{pct:.2}%;background:{color};")}
                                                    on:mouseenter={
                                                        let ev3 = ev2.clone();
                                                        move |_| set_tooltip.set(Some((
                                                            slot_id,
                                                            ev3.event_type.clone(),
                                                            ev3.ts,
                                                            ev3.detail.clone(),
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

                        <div class="tl-time-axis">
                            {labels.iter().map(|l| view! { <span>{l.clone()}</span> }).collect_view()}
                        </div>
                    </div>

                    // Tooltip (shown centred on hover)
                    {move || match tooltip.get() {
                        None => view! { <div class="tl-tooltip tl-tooltip-hidden"></div> }.into_view(),
                        Some((slot, etype, ts, detail)) => view! {
                            <div class="tl-tooltip tl-tooltip-visible">
                                <div class="tl-tt-type">{etype.clone()}</div>
                                <div class="tl-tt-meta">{format!("Slot {slot} · {}", fmt_hhmmss(ts))}</div>
                                {detail.map(|d| view! { <div class="tl-tt-detail">{d}</div> })}
                            </div>
                        }.into_view(),
                    }}

                    // ── Vertical event list (newest first) ───────────────────
                    <div class="tl-event-list">
                        <div class="tl-event-list-header">"Recent Events"</div>
                        {d.events.iter().take(40).map(|ev| {
                            let color = event_color(&ev.event_type);
                            let time_str = fmt_hhmmss(ev.ts);
                            let slot = ev.slot_id.unwrap_or(0);
                            let etype = ev.event_type.clone();
                            let detail = ev.detail.clone().unwrap_or_default();
                            view! {
                                <div class="tl-ev-row">
                                    <span
                                        class="tl-ev-dot"
                                        style={format!("background:{color};")}
                                    ></span>
                                    <span class="tl-ev-time">{time_str}</span>
                                    <span class="tl-ev-slot">{format!("s{slot}")}</span>
                                    <span class="tl-ev-type">{etype}</span>
                                    <span class="tl-ev-detail">{detail}</span>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_view()
            }}
        </div>
    }
}
