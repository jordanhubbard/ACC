use leptos::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Provider {
    id: String,
    kind: String,
    label: String,
    url: String,
    status: String,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProvidersResponse {
    providers: Vec<Provider>,
}

fn status_class(status: &str, enabled: bool) -> &'static str {
    if !enabled || status == "unconfigured" || status == "disabled" {
        "status-dot dot-offline"
    } else {
        "status-dot dot-online"
    }
}

fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "llm"      => "🧠",
        "storage"  => "🗄️",
        "messaging"=> "💬",
        "coding"   => "💻",
        "system"   => "⚙️",
        _          => "🔌",
    }
}

#[component]
pub fn Providers() -> impl IntoView {
    let providers = create_resource(
        || (),
        |_| async move {
            let url = "/api/providers";
            gloo_net::http::Request::get(url)
                .send()
                .await
                .ok()?
                .json::<ProvidersResponse>()
                .await
                .ok()
        },
    );

    view! {
        <div class="providers-panel">
            <div class="panel-header">
                <h2>"Infrastructure Providers"</h2>
            </div>
            <Suspense fallback=move || view! { <p class="loading-text">"Loading providers…"</p> }>
                {move || providers.get().map(|data| match data {
                    None => view! {
                        <div class="error-msg">"Failed to load providers"</div>
                    }.into_view(),
                    Some(resp) => view! {
                        <div class="providers-grid">
                            {resp.providers.iter().map(|p| {
                                let icon = kind_icon(&p.kind);
                                let dot = status_class(&p.status, p.enabled);
                                let label = p.label.clone();
                                let url = p.url.clone();
                                let status = p.status.clone();
                                view! {
                                    <div class="provider-card">
                                        <div class="provider-header">
                                            <span class="provider-icon">{icon}</span>
                                            <span class="provider-label">{label}</span>
                                            <span class={dot}></span>
                                        </div>
                                        <div class="provider-meta">
                                            <span class="provider-status">{status}</span>
                                            {if !url.is_empty() {
                                                view! {
                                                    <span class="provider-url">{url}</span>
                                                }.into_view()
                                            } else {
                                                view! { <span></span> }.into_view()
                                            }}
                                        </div>
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    }.into_view(),
                })}
            </Suspense>
        </div>
    }
}
