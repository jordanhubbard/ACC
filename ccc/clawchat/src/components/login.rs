use leptos::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

#[derive(Deserialize)]
struct LoginResponse {
    ok: bool,
    username: String,
}

async fn validate_credentials(username: &str, token: &str) -> Result<String, String> {
    let body = serde_json::json!({"username": username, "token": token}).to_string();
    let Ok(req) = gloo_net::http::Request::post("/api/auth/login")
        .header("Content-Type", "application/json")
        .body(body)
    else {
        return Err("Failed to build request".into());
    };
    match req.send().await {
        Ok(resp) if resp.ok() => {
            match resp.json::<LoginResponse>().await {
                Ok(r) if r.ok => Ok(r.username),
                _ => Err("Unexpected response from server".into()),
            }
        }
        Ok(_) => Err("Invalid username or token.".into()),
        Err(_) => Err("Could not reach server.".into()),
    }
}

#[component]
pub fn LoginScreen(on_login: impl Fn((String, String)) + 'static + Clone) -> impl IntoView {
    let (tok, set_tok) = create_signal(String::new());
    let (user, set_user) = create_signal(String::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(Option::<String>::None);

    let do_login = {
        let on_login = on_login.clone();
        move || {
            let t = tok.get().trim().to_string();
            let u = user.get().trim().to_string();
            if t.is_empty() || u.is_empty() {
                set_error.set(Some("Username and token are required.".into()));
                return;
            }
            set_loading.set(true);
            set_error.set(None);
            let on_login = on_login.clone();
            spawn_local(async move {
                match validate_credentials(&u, &t).await {
                    Ok(verified_username) => on_login((t, verified_username)),
                    Err(msg) => set_error.set(Some(msg)),
                }
                set_loading.set(false);
            });
        }
    };

    let do_login_click = {
        let d = do_login.clone();
        move |_| d()
    };

    view! {
        <div class="login-screen">
            <div class="login-card">
                <div class="login-logo">"🦞"</div>
                <h1 class="login-title">"ClawChat"</h1>
                <p class="login-sub">"CCC agent communication hub"</p>

                <div class="login-field">
                    <label>"Username"</label>
                    <input
                        type="text"
                        placeholder="your username"
                        prop:value=user
                        attr:disabled=move || if loading.get() { Some("disabled") } else { None }
                        on:input=move |e| set_user.set(event_target_value(&e))
                    />
                </div>

                <div class="login-field">
                    <label>"Token"</label>
                    <input
                        type="password"
                        placeholder="ccc-…"
                        prop:value=tok
                        attr:disabled=move || if loading.get() { Some("disabled") } else { None }
                        on:input=move |e| set_tok.set(event_target_value(&e))
                        on:keydown=move |e| {
                            if e.key() == "Enter" && !loading.get() { do_login(); }
                        }
                    />
                </div>

                {move || error.get().map(|e| view! { <div class="login-error">{e}</div> })}

                <button
                    class="login-btn"
                    attr:disabled=move || if loading.get() { Some("disabled") } else { None }
                    on:click=do_login_click
                >
                    {move || if loading.get() { "Connecting…" } else { "Connect" }}
                </button>

                <p class="login-hint">"Token provided by your admin."</p>
            </div>
        </div>
    }
}
