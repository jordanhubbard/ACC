use leptos::*;

use crate::types::{QueueItem, QueueResponse};

async fn fetch_queue() -> QueueResponse {
    let Ok(resp) = gloo_net::http::Request::get("/api/queue").send().await else {
        return QueueResponse::default();
    };
    resp.json::<QueueResponse>().await.unwrap_or_default()
}

fn priority_class(p: &str) -> &'static str {
    match p {
        "critical" => "prio-critical",
        "high" => "prio-high",
        "medium" => "prio-medium",
        "low" => "prio-low",
        "idea" => "prio-idea",
        _ => "prio-default",
    }
}

fn status_label(s: &str) -> &'static str {
    match s {
        "pending" => "pending",
        "in_progress" | "in-progress" => "in-progress",
        "done" | "completed" => "done",
        "failed" => "failed",
        "blocked" => "blocked",
        _ => "unknown",
    }
}

fn age_display(created_at: &str) -> String {
    // Basic age display — parse ISO timestamp and show relative time
    // For simplicity, just show the date portion
    if let Some(d) = created_at.split('T').next() {
        d.to_string()
    } else {
        created_at.to_string()
    }
}

#[component]
pub fn WorkQueue() -> impl IntoView {
    let (tick, set_tick) = create_signal(0u32);
    let (filter, set_filter) = create_signal(String::new());
    let (expanded_id, set_expanded_id) = create_signal(Option::<String>::None);
    let (show_completed, set_show_completed) = create_signal(false);

    // Poll every 15 seconds
    leptos::spawn_local(async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(15_000).await;
            set_tick.update(|t| *t = t.wrapping_add(1));
        }
    });

    let queue = create_resource(move || tick.get(), |_| fetch_queue());

    let filtered_items = move || {
        let q = queue.get().unwrap_or_default();
        let f = filter.get().to_lowercase();
        let items: Vec<QueueItem> = q
            .items
            .into_iter()
            .filter(|item| {
                if item.priority.as_deref() == Some("idea") {
                    return false; // ideas shown in Idea Incubator
                }
                if f.is_empty() {
                    return true;
                }
                item.title.to_lowercase().contains(&f)
                    || item.id.to_lowercase().contains(&f)
                    || item
                        .assignee
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&f)
                    || item
                        .status
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&f)
            })
            .collect();
        items
    };

    let completed_items = move || {
        let q = queue.get().unwrap_or_default();
        let f = filter.get().to_lowercase();
        q.completed
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                if f.is_empty() {
                    return true;
                }
                item.title.to_lowercase().contains(&f)
                    || item.id.to_lowercase().contains(&f)
            })
            .take(20) // show last 20 completed
            .collect::<Vec<_>>()
    };

    view! {
        <section class="section section-queue">
            <div class="section-header">
                <h2 class="section-title">
                    <span class="section-icon">"▤"</span>
                    "Work Queue"
                    {move || {
                        let count = filtered_items().len();
                        view! { <span class="badge">{count}</span> }
                    }}
                </h2>
                <div class="queue-controls">
                    <input
                        type="text"
                        class="filter-input"
                        placeholder="filter by title, assignee, status..."
                        on:input=move |e| {
                            set_filter.set(event_target_value(&e));
                        }
                    />
                    <label class="toggle-label">
                        <input
                            type="checkbox"
                            on:change=move |e| {
                                set_show_completed
                                    .set(
                                        event_target_checked(&e),
                                    );
                            }
                        />
                        " show completed"
                    </label>
                </div>
            </div>

            <div class="queue-table-wrap">
                <table class="queue-table">
                    <thead>
                        <tr>
                            <th>"ID"</th>
                            <th>"Title"</th>
                            <th>"Priority"</th>
                            <th>"Assignee"</th>
                            <th>"Status"</th>
                            <th>"Age"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let items = filtered_items();
                            if items.is_empty() {
                                return view! {
                                    <tr>
                                        <td colspan="6" class="empty-row">"No items"</td>
                                    </tr>
                                }
                                    .into_view();
                            }
                            items
                                .into_iter()
                                .map(|item| {
                                    let id = item.id.clone();
                                    // Each reactive closure needs its own clone to avoid
                                    // the borrow-after-move compile error.
                                    let id_click   = id.clone();
                                    let id_class   = id.clone();
                                    let id_detail  = id.clone();
                                    let id_display = id.clone();
                                    let prio = item
                                        .priority
                                        .as_deref()
                                        .unwrap_or("medium")
                                        .to_string();
                                    let pclass = priority_class(&prio);
                                    let status = item
                                        .status
                                        .as_deref()
                                        .unwrap_or("pending")
                                        .to_string();
                                    let slabel = status_label(&status).to_string();
                                    let age = item
                                        .created_at
                                        .as_deref()
                                        .map(age_display)
                                        .unwrap_or_default();
                                    let assignee = item
                                        .assignee
                                        .clone()
                                        .unwrap_or_default();
                                    let title = item.title.clone();
                                    let body = item.body.clone().unwrap_or_default();
                                    view! {
                                        <tr
                                            class=move || {
                                                if expanded_id.get().as_deref()
                                                    == Some(id_class.as_str())
                                                {
                                                    "queue-row expanded"
                                                } else {
                                                    "queue-row"
                                                }
                                            }
                                            on:click=move |_| {
                                                let cur = expanded_id.get();
                                                if cur.as_deref() == Some(id_click.as_str()) {
                                                    set_expanded_id.set(None);
                                                } else {
                                                    set_expanded_id.set(Some(id_click.clone()));
                                                }
                                            }
                                        >
                                            <td class="col-id">
                                                <span class="item-id">{id_display}</span>
                                            </td>
                                            <td class="col-title">{title}</td>
                                            <td class="col-prio">
                                                <span class=format!("prio-badge {pclass}")>
                                                    {prio}
                                                </span>
                                            </td>
                                            <td class="col-assignee">{assignee}</td>
                                            <td class="col-status">
                                                <span class=format!("status-badge status-{slabel}")>
                                                    {slabel}
                                                </span>
                                            </td>
                                            <td class="col-age">{age}</td>
                                        </tr>
                                        {move || {
                                            if expanded_id.get().as_deref()
                                                == Some(id_detail.as_str())
                                            {
                                                view! {
                                                    <tr class="detail-row">
                                                        <td colspan="6">
                                                            <div class="item-detail">
                                                                <pre class="item-body">
                                                                    {body.clone()}
                                                                </pre>
                                                            </div>
                                                        </td>
                                                    </tr>
                                                }
                                                .into_view()
                                            } else {
                                                view! { <></> }.into_view()
                                            }
                                        }}
                                    }
                                })
                                .collect::<Vec<_>>()
                                .into_view()
                        }}
                    </tbody>
                </table>
            </div>

            {move || {
                if show_completed.get() {
                    let items = completed_items();
                    view! {
                        <div class="completed-section">
                            <h3 class="subsection-title">"Recently Completed"</h3>
                            <table class="queue-table completed-table">
                                <thead>
                                    <tr>
                                        <th>"ID"</th>
                                        <th>"Title"</th>
                                        <th>"Assignee"</th>
                                        <th>"Resolution"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {items
                                        .into_iter()
                                        .map(|item| {
                                            view! {
                                                <tr class="queue-row done">
                                                    <td class="col-id">
                                                        {item.id.clone()}
                                                    </td>
                                                    <td>{item.title.clone()}</td>
                                                    <td>
                                                        {item.assignee.clone().unwrap_or_default()}
                                                    </td>
                                                    <td class="resolution">
                                                        {item
                                                            .resolution
                                                            .as_deref()
                                                            .unwrap_or("")
                                                            .chars()
                                                            .take(80)
                                                            .collect::<String>()}
                                                    </td>
                                                </tr>
                                            }
                                        })
                                        .collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }
                        .into_view()
                } else {
                    view! { <></> }.into_view()
                }
            }}
        </section>
    }
}
