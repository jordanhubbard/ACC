//! Fleet task worker — polls /api/tasks, claims atomically, executes in AgentFS workspace.
//!
//! Work tasks and review tasks are executed via the native Anthropic agentic loop in sdk.rs.
//! Phase_commit tasks run git to push approved work to a branch.
//! Multiple agents run this concurrently; the server's SQL atomic claim prevents double-work.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use tokio::process::Command;
use tokio::sync::Notify;
use tokio::time::sleep;
use serde_json::Value;
use acc_client::Client;
use acc_model::{CreateTaskRequest, HeartbeatRequest, ReviewResult, TaskStatus, TaskType};
use crate::config::Config;
use crate::peers;

/// Error type returned by `run_git_phase_commit`.
///
/// * `Transient` — a retriable network/infrastructure hiccup; the caller
///   should silently requeue (no investigation task) and let the next
///   dispatch cycle retry.
/// * `Hard` — a permanent failure (auth rejected, non-fast-forward, local
///   git error, …); the caller should file an investigation task so a human
///   can look into it.
#[derive(Debug)]
enum PhaseCommitError {
    Transient(String),
    Hard(String),
}

impl std::fmt::Display for PhaseCommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PhaseCommitError::Transient(msg) => write!(f, "(transient) {msg}"),
            PhaseCommitError::Hard(msg) => write!(f, "{msg}"),
        }
    }
}

const POLL_IDLE: Duration = Duration::from_secs(30);
const POLL_BUSY: Duration = Duration::from_secs(5);

/// Hard cap on a single review's agentic loop. Without this, a stuck
/// model call can hold a claim indefinitely (observed: 4h+ claims that
/// never complete, blocking the whole fleet via `count_active_tasks`).
const REVIEW_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Hard cap on a work task's agentic loop. Longer than reviews because
/// real implementation can legitimately take an hour+, but must be
/// bounded.
const WORK_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);

/// After this agent completes or unclaims a task, skip re-claiming it
/// for this long. Breaks the re-claim loop where an agent instantly
/// grabs back a task it just released (observed 2026-04-24: unclaim
/// reversed by same-agent re-claim within 15s on every attempt).
const RECLAIM_COOLDOWN: Duration = Duration::from_secs(15 * 60);

/// Keepalive heartbeat interval while a long-running task is in
/// flight. Without this, the hub sees silence for the full
/// REVIEW_TIMEOUT (30min) or WORK_TIMEOUT (2h) and may treat the
/// agent as dead even though it's actively working (CCC-79d).
const TASK_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

/// Spawn a background task that posts /api/heartbeat/{agent} every
/// TASK_KEEPALIVE_INTERVAL until the returned sender is dropped or
/// signaled. Fire-and-forget on the network: if a heartbeat POST
/// fails, the next interval tries again.
fn spawn_keepalive(
    cfg: Config,
    client: Client,
    note: String,
) -> tokio::sync::oneshot::Sender<()> {
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TASK_KEEPALIVE_INTERVAL);
        // Skip the immediate first tick; heartbeat is for long-running
        // gaps, not for the moment after claim.
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let req = HeartbeatRequest {
                        ts: Some(chrono::Utc::now()),
                        status: Some("ok".into()),
                        note: Some(note.clone()),
                        host: Some(cfg.host.clone()),
                        ssh_user: Some(cfg.ssh_user.clone()),
                        ssh_host: Some(cfg.ssh_host.clone()),
                        ssh_port: Some(cfg.ssh_port as u64),
                    };
                    let _ = client.items().heartbeat(&cfg.agent_name, &req).await;
                }
                _ = &mut stop_rx => break,
            }
        }
    });
    stop_tx
}

/// Per-process cache of `(task_id, released_at)`. Keeps the last
/// `RECLAIM_COOLDOWN` worth of finished tasks so the poll loop can
/// skip them.
fn recent_done() -> &'static Mutex<HashMap<String, Instant>> {
    static CELL: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn mark_done(task_id: &str) {
    if let Ok(mut m) = recent_done().lock() {
        // GC entries older than the cooldown window
        let now = Instant::now();
        m.retain(|_, t| now.duration_since(*t) < RECLAIM_COOLDOWN);
        m.insert(task_id.to_string(), now);
    }
}

fn in_cooldown(task_id: &str) -> bool {
    if let Ok(m) = recent_done().lock() {
        if let Some(t) = m.get(task_id) {
            return t.elapsed() < RECLAIM_COOLDOWN;
        }
    }
    false
}

pub async fn run(args: &[String]) {
    let max_concurrent: usize = args.iter()
        .find(|a| a.starts_with("--max="))
        .and_then(|a| a[6..].parse().ok())
        .or_else(|| std::env::var("ACC_MAX_TASKS_PER_AGENT").ok().and_then(|v| v.parse().ok()))
        .unwrap_or(2);

    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => { eprintln!("[tasks] config error: {e}"); std::process::exit(1); }
    };
    if cfg.agent_name.is_empty() {
        eprintln!("[tasks] AGENT_NAME not set"); std::process::exit(1);
    }

    let _ = std::fs::create_dir_all(cfg.acc_dir.join("logs"));
    log(&cfg, &format!("starting (agent={}, hub={}, max_concurrent={}, pair_programming={})",
        cfg.agent_name, cfg.acc_url, max_concurrent, cfg.pair_programming));

    let client = match Client::new(&cfg.acc_url, &cfg.acc_token) {
        Ok(c) => c,
        Err(e) => { eprintln!("[tasks] http client: {e}"); std::process::exit(1); }
    };

    // Recovery: unclaim any tasks the server still attributes to this
    // agent. They were claimed by a previous process that died (restart,
    // crash, kill); we have no in-memory state for them and cannot
    // resume work in-flight. Without this, a restarted agent sees
    // active >= max_concurrent and never polls work.
    cleanup_stale_claims(&cfg, &client).await;

    // Bus subscriber: wakes the poll loop immediately on dispatch nudge/assign
    let nudge = Arc::new(Notify::new());
    {
        let cfg2 = cfg.clone();
        let client2 = client.clone();
        let nudge2 = nudge.clone();
        tokio::spawn(bus_subscriber(cfg2, client2, nudge2));
    }

    loop {
        if is_quenched(&cfg) {
            log(&cfg, "quenched — sleeping");
            sleep(POLL_IDLE).await;
            continue;
        }

        let active = count_active_tasks(&cfg, &client).await;
        let at_work_cap = active >= max_concurrent;

        // Fetch online peers once per cycle (used by all three polls)
        let online_peers = peers::list_peers(&cfg, &client).await;
        let mut claimed = false;

        // ── Poll 1: work tasks (skipped when at capacity) ───────────────────
        if !at_work_cap {
            let fetch_limit = ((max_concurrent - active) * 5).max(10);
            match fetch_open_tasks(&cfg, &client, fetch_limit, "work").await {
                Err(e) => {
                    log(&cfg, &format!("fetch failed: {e}"));
                    sleep(POLL_IDLE).await;
                    continue;
                }
                Ok(open_tasks) => {
                    for task in &open_tasks {
                        let task_id = task["id"].as_str().unwrap_or("").to_string();
                        if task_id.is_empty() { continue; }
                        if in_cooldown(&task_id) { continue; }

                        let preferred = task["metadata"]["preferred_executor"].as_str().unwrap_or("");
                        if !preferred.is_empty()
                            && preferred != cfg.agent_name.as_str()
                            && online_peers.iter().any(|p| p == preferred)
                        {
                            log(&cfg, &format!("skipping {task_id} — preferred by {preferred} (online)"));
                            continue;
                        }

                        match claim_task(&cfg, &client, &task_id).await {
                            Ok(claimed_task) => {
                                log(&cfg, &format!("claimed task {task_id}: {}", claimed_task["title"].as_str().unwrap_or("")));
                                let cfg2 = cfg.clone();
                                let client2 = client.clone();
                                let task2 = claimed_task.clone();
                                let peers2 = online_peers.clone();
                                tokio::spawn(async move {
                                    execute_task(&cfg2, &client2, &task2, &peers2).await;
                                });
                                claimed = true;
                                break;
                            }
                            Err(409) | Err(423) => { /* already claimed or blocked, try next */ }
                            Err(429) => {
                                log(&cfg, "at capacity (server side)");
                                break;
                            }
                            Err(e) => {
                                log(&cfg, &format!("claim error {e} for {task_id}"));
                            }
                        }
                    }
                }
            }
        }

        // ── Poll 2: review tasks (runs even when at work capacity) ──────────
        // Reviews are bounded to 1 concurrent per agent (max_concurrent cap
        // applies separately; reviews are lighter than full work tasks).
        if !claimed {
            if let Ok(review_tasks) = fetch_open_tasks(&cfg, &client, 10, "review").await {
                for task in &review_tasks {
                    let task_id = task["id"].as_str().unwrap_or("").to_string();
                    if task_id.is_empty() { continue; }
                    if in_cooldown(&task_id) { continue; }

                    let preferred = task["metadata"]["preferred_executor"].as_str().unwrap_or("");
                    if !preferred.is_empty()
                        && preferred != cfg.agent_name.as_str()
                        && online_peers.iter().any(|p| p == preferred)
                    {
                        continue;
                    }

                    match claim_task(&cfg, &client, &task_id).await {
                        Ok(claimed_task) => {
                            log(&cfg, &format!("claimed review {task_id}"));
                            let cfg2 = cfg.clone();
                            let client2 = client.clone();
                            let task2 = claimed_task.clone();
                            tokio::spawn(async move {
                                execute_review_task(&cfg2, &client2, &task2).await;
                            });
                            claimed = true;
                            break;
                        }
                        Err(409) | Err(423) => {}
                        Err(e) => { log(&cfg, &format!("review claim error {e} for {task_id}")); }
                    }
                }
            }
        }

        // ── Poll 3: phase_commit tasks ──────────────────────────────────────
        if !claimed && !at_work_cap {
            if let Ok(phase_tasks) = fetch_open_tasks(&cfg, &client, 5, "phase_commit").await {
                for task in &phase_tasks {
                    let task_id = task["id"].as_str().unwrap_or("").to_string();
                    if task_id.is_empty() { continue; }
                    if in_cooldown(&task_id) { continue; }

                    match claim_task(&cfg, &client, &task_id).await {
                        Ok(claimed_task) => {
                            log(&cfg, &format!("claimed phase_commit {task_id}"));
                            let cfg2 = cfg.clone();
                            let client2 = client.clone();
                            let task2 = claimed_task.clone();
                            tokio::spawn(async move {
                                execute_phase_commit_task(&cfg2, &client2, &task2).await;
                            });
                            claimed = true;
                            break;
                        }
                        Err(409) | Err(423) => {}
                        Err(e) => { log(&cfg, &format!("phase_commit claim error {e} for {task_id}")); }
                    }
                }
            }
        }

        if at_work_cap && !claimed {
            log(&cfg, &format!("at work capacity ({}/{}), waiting", active, max_concurrent));
        }

        if claimed {
            sleep(POLL_BUSY).await;
        } else {
            // Wait for idle timeout OR a dispatch nudge — whichever comes first
            tokio::select! {
                _ = sleep(POLL_IDLE) => {}
                _ = nudge.notified() => {
                    log(&cfg, "woke early — dispatch nudge received");
                }
            }
        }
    }
}

// ── Startup recovery — release stale claims from previous process ───────────

async fn cleanup_stale_claims(cfg: &Config, client: &Client) {
    let stale = match client
        .tasks()
        .list()
        .status(TaskStatus::Claimed)
        .agent(cfg.agent_name.clone())
        .send()
        .await
    {
        Ok(v) => v,
        Err(e) => {
            log(cfg, &format!("startup recovery: failed to list own claims: {e}"));
            return;
        }
    };
    if stale.is_empty() {
        return;
    }
    log(
        cfg,
        &format!("startup recovery: releasing {} stale claim(s) from previous process", stale.len()),
    );
    for t in &stale {
        let _ = client.tasks().unclaim(&t.id, Some(&cfg.agent_name)).await;
        // Populate cooldown so we don't immediately re-claim. Other
        // agents (who haven't been running this task) can still pick it up.
        mark_done(&t.id);
    }
}

// ── Bus subscriber — wakes poll loop on dispatch nudge/assign ─────────────────

async fn bus_subscriber(cfg: Config, client: Client, nudge: Arc<Notify>) {
    loop {
        match subscribe_bus(&cfg, &client, &nudge).await {
            Ok(()) => {}
            Err(e) => {
                log(&cfg, &format!("[bus] disconnected: {e}, reconnecting in 5s"));
                sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn subscribe_bus(cfg: &Config, client: &Client, nudge: &Arc<Notify>) -> Result<(), String> {
    use futures_util::StreamExt;
    let stream = client.bus().stream();
    tokio::pin!(stream);
    while let Some(msg) = stream.next().await {
        let msg = msg.map_err(|e| e.to_string())?;
        let kind = msg.kind.as_deref().unwrap_or("");
        let to = msg.to.as_deref().unwrap_or("");
        let is_directed_to_us = to == cfg.agent_name;
        let is_broadcast = to.is_empty() || to == "null";

        if kind == "tasks:dispatch_nudge" && (is_directed_to_us || is_broadcast) {
            nudge.notify_one();
        } else if kind == "tasks:dispatch_assigned" && is_directed_to_us {
            nudge.notify_one();
        }
    }
    Ok(())
}

// ── Fetching / claiming ───────────────────────────────────────────────────────

async fn fetch_open_tasks(_cfg: &Config, client: &Client, limit: usize, task_type: &str) -> Result<Vec<Value>, String> {
    let tt = parse_task_type(task_type).unwrap_or(TaskType::Work);
    let tasks = client
        .tasks()
        .list()
        .status(TaskStatus::Open)
        .task_type(tt)
        .limit(limit.max(1) as u32)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(tasks.into_iter().map(to_value).collect())
}

async fn count_active_tasks(cfg: &Config, client: &Client) -> usize {
    match client
        .tasks()
        .list()
        .status(TaskStatus::Claimed)
        .agent(cfg.agent_name.clone())
        .send()
        .await
    {
        Ok(tasks) => tasks.len(),
        Err(_) => 0,
    }
}

async fn claim_task(cfg: &Config, client: &Client, task_id: &str) -> Result<Value, u16> {
    match client.tasks().claim(task_id, &cfg.agent_name).await {
        Ok(task) => Ok(to_value(task)),
        Err(e) => Err(e.status_code().unwrap_or(500)),
    }
}

// ── Work task execution ───────────────────────────────────────────────────────

async fn execute_task(cfg: &Config, client: &Client, task: &Value, online_peers: &[String]) {
    let task_id = task["id"].as_str().unwrap_or("unknown");
    let title = task["title"].as_str().unwrap_or("(no title)");
    let project_id = task["project_id"].as_str().unwrap_or("");

    log(cfg, &format!("executing task {task_id}: {title}"));

    let workspace = resolve_workspace(cfg, client, project_id, task_id).await;
    let _ = std::fs::create_dir_all(&workspace);

    let ctx_path = workspace.join(".task-context.json");
    let _ = std::fs::write(&ctx_path, task.to_string());

    let description = task["description"].as_str().unwrap_or("");
    let prompt = format!(
        "You are an autonomous coding agent. Your task:\n\
         \n\
         Title: {title}\n\
         \n\
         Description:\n\
         {description}\n\
         \n\
         You are in a git working directory with the project source. Apply the \
         requested changes by calling `str_replace_editor` (for file edits) or \
         `bash` (to run scripts, tests, etc.). The completion of this task is \
         verified by `git diff` against your edits — a written description that \
         doesn't actually modify files counts as a failed task.\n\
         \n\
         When the edits are applied, summarize in 1-3 sentences what you changed."
    );

    // CCC-79d: heartbeat every 60s while the agentic loop runs so the
    // hub doesn't read silence-for-2h as agent-dead.
    let ka_stop = spawn_keepalive(
        cfg.clone(),
        client.clone(),
        format!("working task {task_id}"),
    );
    let result = match tokio::time::timeout(
        WORK_TIMEOUT,
        crate::sdk::run_agent(&prompt, &workspace),
    ).await {
        Ok(r) => r,
        Err(_) => Err(format!("timeout after {}m", WORK_TIMEOUT.as_secs() / 60)),
    };
    let _ = ka_stop.send(());

    match result {
        Ok(output) => {
            if cfg.pair_programming {
                submit_for_review(cfg, client, task, &output, online_peers).await;
            } else {
                complete_task(cfg, client, task_id, &output).await;
                log(cfg, &format!("completed {task_id}"));
            }
        }
        Err(e) => {
            log(cfg, &format!("task {task_id} failed: {e}"));
            unclaim_task(cfg, client, task_id).await;
        }
    }
    mark_done(task_id);
}


// ── Pair programming: submit for review ──────────────────────────────────────

async fn submit_for_review(cfg: &Config, client: &Client, task: &Value, output: &str, online_peers: &[String]) {
    let task_id = task["id"].as_str().unwrap_or("");
    let project_id = task["project_id"].as_str().unwrap_or("");
    let title = task["title"].as_str().unwrap_or("(task)");
    let priority = task["priority"].as_i64().unwrap_or(2);
    let phase = task["phase"].as_str();

    // Work is done — complete it first
    complete_task(cfg, client, task_id, output).await;

    // Pick reviewer: first online peer that is not me
    let reviewer = online_peers.iter()
        .find(|p| p.as_str() != cfg.agent_name.as_str())
        .map(|s| s.as_str())
        .unwrap_or("");

    let summary = &output[..output.len().min(2000)];
    let mut meta = serde_json::json!({"work_output_summary": summary});
    if !reviewer.is_empty() {
        meta["preferred_executor"] = Value::String(reviewer.to_string());
    }

    let review_desc = format!(
        "Review the completed work for task '{title}' (ID: {task_id}).\n\nWorker summary:\n{summary}\n\nCheck the shared project workspace for changes."
    );

    let req = CreateTaskRequest {
        project_id: project_id.to_string(),
        title: format!("Review: {title}"),
        description: Some(review_desc),
        priority: Some(priority),
        task_type: Some(TaskType::Review),
        review_of: Some(task_id.to_string()),
        phase: phase.map(|p| p.to_string()),
        metadata: Some(meta),
        ..Default::default()
    };

    match client.tasks().create(&req).await {
        Ok(review) => {
            log(cfg, &format!("submitted {task_id} for review → {} (reviewer: {})", review.id,
                if reviewer.is_empty() { "any" } else { reviewer }));
        }
        Err(e) => log(cfg, &format!("failed to create review task: {e}")),
    }
}

// ── Review task execution ─────────────────────────────────────────────────────

async fn execute_review_task(cfg: &Config, client: &Client, task: &Value) {
    let task_id = task["id"].as_str().unwrap_or("unknown");
    let review_of_id = task["review_of"].as_str().unwrap_or("");
    let phase = task["phase"].as_str().unwrap_or("");

    log(cfg, &format!("executing review {task_id} (reviewing {review_of_id})"));

    // Fetch original task to get project_id
    let project_id = fetch_task_project_id(cfg, client, review_of_id, task).await;

    let workspace = resolve_workspace(cfg, client, &project_id, "").await;
    let _ = std::fs::create_dir_all(&workspace);

    let work_summary = task["metadata"]["work_output_summary"].as_str().unwrap_or("");
    let ctx = serde_json::json!({
        "review_task": task,
        "review_of_id": review_of_id,
        "work_output_summary": work_summary,
    });
    let _ = std::fs::write(workspace.join(".review-context.json"), ctx.to_string());

    let review_prompt = format!(
        "You are a code reviewer in an automated pair-programming workflow.\n\n\
         Original task: {title}\n\
         Original task ID: {review_of_id}\n\
         Worker's own summary: {summary}\n\n\
         The working directory contains the project files written by the worker.\n\n\
         Review this work and respond with ONLY a single valid JSON object — no prose, no markdown:\n\
         {{\n\
           \"verdict\": \"approved\",\n\
           \"reason\": \"<one sentence>\",\n\
           \"gaps\": [\n\
             {{\n\
               \"title\": \"<short task title for the gap>\",\n\
               \"description\": \"<what still needs to be done and why>\",\n\
               \"priority\": 1\n\
             }}\n\
           ]\n\
         }}\n\n\
         Replace \"approved\" with \"rejected\" if there is a serious defect that must be fixed \
         before this phase can be committed. Gaps may be filed even for approved work.\n\n\
         Check for: (1) task completion, (2) consistency with existing code style and architecture, \
         (3) any CI/CD blockers such as missing tests or broken imports, \
         (4) remaining gaps the original task left unaddressed.",
        title = task["title"].as_str().unwrap_or("(task)"),
        summary = &work_summary[..work_summary.len().min(2000)],
    );

    // CCC-79d: heartbeat every 60s while the review's agentic loop
    // runs. Without this, the hub sees silence for the full
    // REVIEW_TIMEOUT (30min) and may treat the agent as dead.
    let ka_stop = spawn_keepalive(
        cfg.clone(),
        client.clone(),
        format!("reviewing task {task_id}"),
    );
    let review_output = match tokio::time::timeout(
        REVIEW_TIMEOUT,
        crate::sdk::run_agent(&review_prompt, &workspace),
    ).await {
        Ok(r) => r,
        Err(_) => Err(format!("timeout after {}m", REVIEW_TIMEOUT.as_secs() / 60)),
    };
    let _ = ka_stop.send(());

    let (verdict, reason, gaps) = match review_output {
        Ok(out) => parse_review_output(&out),
        Err(e) => {
            log(cfg, &format!("review subprocess failed: {e}"));
            ("rejected".to_string(), format!("subprocess failed: {e}"), vec![])
        }
    };

    // File gap tasks
    for gap in &gaps {
        create_gap_task(cfg, client, &project_id, phase, task_id, gap).await;
    }

    // Record verdict on the original work task
    if !review_of_id.is_empty() {
        set_review_result_on_task(cfg, client, review_of_id, &verdict, &reason).await;
    }

    complete_task(cfg, client, task_id, &format!("verdict: {verdict}, reason: {reason}")).await;
    mark_done(task_id);
    log(cfg, &format!("review {task_id} done: {verdict} ({} gaps filed)", gaps.len()));
}

async fn fetch_task_project_id(_cfg: &Config, client: &Client, task_id: &str, fallback_task: &Value) -> String {
    if task_id.is_empty() {
        return fallback_task["project_id"].as_str().unwrap_or("").to_string();
    }
    match client.tasks().get(task_id).await {
        Ok(task) => task.project_id,
        Err(_) => fallback_task["project_id"].as_str().unwrap_or("").to_string(),
    }
}

fn parse_review_output(output: &str) -> (String, String, Vec<Value>) {
    let start = output.find('{').unwrap_or(output.len());
    let end = output.rfind('}').map(|i| i + 1).unwrap_or(output.len());
    if start >= end {
        return ("rejected".to_string(), "unparseable output".to_string(), vec![]);
    }
    match serde_json::from_str::<Value>(&output[start..end]) {
        Ok(v) => {
            let verdict = v["verdict"].as_str().unwrap_or("rejected").to_string();
            let reason = v["reason"].as_str().unwrap_or("").to_string();
            let gaps = v["gaps"].as_array().cloned().unwrap_or_default();
            (verdict, reason, gaps)
        }
        Err(_) => ("rejected".to_string(), "unparseable output".to_string(), vec![]),
    }
}

async fn create_gap_task(cfg: &Config, client: &Client, project_id: &str, phase: &str, review_task_id: &str, gap: &Value) {
    let title = gap["title"].as_str().unwrap_or("Gap task").to_string();
    let description = gap["description"].as_str().unwrap_or("").to_string();
    let priority = gap["priority"].as_i64().unwrap_or(2);

    let req = CreateTaskRequest {
        project_id: project_id.to_string(),
        title: title.clone(),
        description: Some(description),
        priority: Some(priority),
        task_type: Some(TaskType::Work),
        phase: (!phase.is_empty()).then(|| phase.to_string()),
        metadata: Some(serde_json::json!({"spawned_by_review": review_task_id})),
        ..Default::default()
    };

    match client.tasks().create(&req).await {
        Ok(task) => log(cfg, &format!("filed gap task {}: {title}", task.id)),
        Err(e) => log(cfg, &format!("failed to create gap task: {e}")),
    }
}

async fn set_review_result_on_task(cfg: &Config, client: &Client, task_id: &str, verdict: &str, reason: &str) {
    let result = match verdict {
        "approved" => ReviewResult::Approved,
        _ => ReviewResult::Rejected,
    };
    let _ = client
        .tasks()
        .review_result(task_id, result, Some(&cfg.agent_name), Some(reason))
        .await;
}

// ── Phase commit task execution ───────────────────────────────────────────────

async fn execute_phase_commit_task(cfg: &Config, client: &Client, task: &Value) {
    let task_id = task["id"].as_str().unwrap_or("unknown");
    let project_id = task["project_id"].as_str().unwrap_or("");
    let phase = task["phase"].as_str().unwrap_or("unknown");

    log(cfg, &format!("executing phase_commit {task_id}: phase={phase}"));

    let workspace = resolve_workspace(cfg, client, project_id, "").await;
    let branch = format!("phase/{phase}");
    let n_blocked = task["blocked_by"].as_array().map(|a| a.len()).unwrap_or(0);
    let commit_msg = format!("phase commit: {phase} ({n_blocked} tasks reviewed and approved)");

    match run_git_phase_commit(&workspace, &branch, &commit_msg).await {
        Ok(out) => {
            log(cfg, &format!("phase_commit {task_id}: pushed {branch}"));

            // Drift-fix #2: phase branches were piling up on origin
            // without ever being merged back to main (852 unmerged
            // commits on phase/milestone observed). Try a fast-forward
            // merge of phase/<phase> back into main and push. If the
            // FF can't happen (e.g. someone landed a PR on main since
            // we branched), leave main alone and surface for human
            // review — never do non-FF merges automatically.
            //
            // Use run_git_merge_to_main_inner here rather than the
            // run_git_merge_to_main wrapper so that both git sequences
            // (phase-commit and merge-to-main) can be composed under a
            // single workspace mutex guard in the future without
            // triggering a re-entrant deadlock on tokio::sync::Mutex.
            let merge_outcome = run_git_merge_to_main_inner(&workspace, &branch).await;
            match &merge_outcome {
                Ok(s) => log(cfg, &format!("phase_commit {task_id}: merged {branch} → main ({s})")),
                Err(e) => log(cfg, &format!("phase_commit {task_id}: merge to main skipped/failed: {e}")),
            }

            // CCC-tk0: this is the milestone-commit task. Now that the
            // AgentFS state is committed and pushed to git, mark the
            // project's AgentFS as clean. Server-side dirty bit gets
            // re-set the next time any task in this project completes.
            if !project_id.is_empty() {
                let path = format!("/api/projects/{project_id}/clean");
                if let Err(e) = client.request_json("POST", &path, None).await {
                    log(cfg, &format!("phase_commit {task_id}: /clean failed: {e} (push succeeded; bit will need manual reset)"));
                } else {
                    log(cfg, &format!("phase_commit {task_id}: marked project {project_id} clean"));
                }
            }
            let summary = match merge_outcome {
                Ok(s) => format!("pushed {branch}: {out}; main: {s}"),
                Err(e) => format!("pushed {branch}: {out}; main: not merged ({e})"),
            };
            complete_task(cfg, client, task_id, &summary).await;
        }
        Err(PhaseCommitError::Transient(e)) => {
            // Transient network failure — silently requeue so the next
            // dispatch cycle retries.  Do NOT file an investigation task;
            // that would flood the queue with noise on flaky networks.
            log(cfg, &format!("phase_commit {task_id} transient git failure (requeueing): {e}"));
            // Drift-fix #4: tell the server this attempt failed so the
            // dispatch loop can stop auto-filing if we hit 3 in a row.
            if !project_id.is_empty() {
                let path = format!("/api/projects/{project_id}/phase-commit-failed");
                let body = serde_json::json!({"reason": e});
                if let Err(re) = client.request_json("POST", &path, Some(&body)).await {
                    log(cfg, &format!("phase_commit {task_id}: failure-report POST failed: {re}"));
                }
            }
            unclaim_task(cfg, client, task_id).await;
        }
        Err(PhaseCommitError::Hard(e)) => {
            log(cfg, &format!("phase_commit {task_id} git failed: {e}"));
            // Drift-fix #4: tell the server this attempt failed so the
            // dispatch loop can stop auto-filing if we hit 3 in a row.
            if !project_id.is_empty() {
                let path = format!("/api/projects/{project_id}/phase-commit-failed");
                let body = serde_json::json!({"reason": e});
                if let Err(re) = client.request_json("POST", &path, Some(&body)).await {
                    log(cfg, &format!("phase_commit {task_id}: failure-report POST failed: {re}"));
                }
            }
            unclaim_task(cfg, client, task_id).await;
            // File investigation task for hard (non-retriable) failures.
            let req = CreateTaskRequest {
                project_id: project_id.to_string(),
                title: format!("Investigate git failure: phase {phase}"),
                description: Some(format!("Phase commit failed for {task_id}: {e}")),
                priority: Some(0),
                task_type: Some(TaskType::Work),
                ..Default::default()
            };
            let _ = client.tasks().create(&req).await;
        }
    }
    mark_done(task_id);
}

/// Inner (mutex-free) implementation of the merge-to-main sequence.
///
/// This function contains all the git operations without acquiring any
/// workspace mutex itself, making it safe to call from a context that
/// already holds a workspace lock (e.g. from within `run_git_phase_commit`
/// once that function is refactored to hold a per-workspace mutex).
///
/// Callers that do NOT already hold the workspace mutex should call the
/// thin wrapper [`run_git_merge_to_main`] instead, which exists solely to
/// provide a named entry-point that documents the locking contract.
///
/// Sequence:
///   git fetch origin --quiet
///   git checkout main
///   git pull --ff-only origin main          # land any other commits
///   git merge --ff-only <branch>            # FF main forward
///   git push origin main
///
/// All steps after `git fetch` are best-effort: if any step fails we
/// stop and return Err with stderr context. The phase branch remains
/// pushed; only the main update is skipped.
async fn run_git_merge_to_main_inner(workspace: &PathBuf, branch: &str) -> Result<String, PhaseCommitError> {
    let ws = workspace.to_str().unwrap_or(".");

    let fetch = Command::new("git").args(["-C", ws, "fetch", "origin", "--quiet"])
        .output().await.map_err(|e| PhaseCommitError::Hard(format!("git fetch: {e}")))?;
    if !fetch.status.success() {
        return Err(PhaseCommitError::Hard(format!("fetch: {}", String::from_utf8_lossy(&fetch.stderr).trim())));
    }

    let checkout = Command::new("git").args(["-C", ws, "checkout", "main"])
        .output().await.map_err(|e| PhaseCommitError::Hard(format!("git checkout main: {e}")))?;
    if !checkout.status.success() {
        return Err(PhaseCommitError::Hard(format!("checkout main: {}", String::from_utf8_lossy(&checkout.stderr).trim())));
    }

    let pull = Command::new("git").args(["-C", ws, "pull", "--ff-only", "origin", "main", "--quiet"])
        .output().await.map_err(|e| PhaseCommitError::Hard(format!("git pull main: {e}")))?;
    if !pull.status.success() {
        let stderr = String::from_utf8_lossy(&pull.stderr).to_string();
        // diverged main is a real possibility if main moved past our last
        // pull; abort safely.
        return Err(PhaseCommitError::Hard(format!("pull --ff-only: {stderr}")));
    }

    let merge = Command::new("git").args(["-C", ws, "merge", "--ff-only", branch, "--quiet"])
        .output().await.map_err(|e| PhaseCommitError::Hard(format!("git merge: {e}")))?;
    if !merge.status.success() {
        let stderr = String::from_utf8_lossy(&merge.stderr).to_string();
        return Err(PhaseCommitError::Hard(format!("merge --ff-only {branch}: {stderr}")));
    }

    let push = tokio::time::timeout(
        Duration::from_secs(600),
        Command::new("git").args(["-C", ws, "push", "origin", "main"]).output(),
    ).await
    .map_err(|_| PhaseCommitError::Transient("git push main timed out".to_string()))?
    .map_err(|e| PhaseCommitError::Hard(format!("git push main: {e}")))?;
    if !push.status.success() {
        return Err(PhaseCommitError::Hard(format!("push main: {}", String::from_utf8_lossy(&push.stderr).trim())));
    }
    Ok("fast-forwarded".to_string())
}

/// Drift-fix #2: after pushing phase/<phase>, try to fast-forward main.
///
/// This is a thin wrapper around [`run_git_merge_to_main_inner`] for callers
/// that do **not** already hold a workspace mutex.  The separation exists to
/// prevent re-entrant deadlocks: if this function and `run_git_phase_commit`
/// were ever composed under a single `tokio::sync::Mutex` guard (which is
/// not reentrant), any code path that called this wrapper while already
/// holding the guard would deadlock.  Factoring the logic into the inner
/// function allows such composed callers to call `run_git_merge_to_main_inner`
/// directly without re-acquiring the lock.
async fn run_git_merge_to_main(workspace: &PathBuf, branch: &str) -> Result<String, PhaseCommitError> {
    run_git_merge_to_main_inner(workspace, branch).await
}

/// Return true when a `git push` stderr looks like a transient network
/// hiccup that is worth retrying (connection-level failures, remote
/// service unavailability).  Auth failures and non-fast-forward
/// rejections are *permanent* — retrying them wastes time and may even
/// trigger rate-limiting, so they are not considered transient.
fn is_transient_network_error(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    // Permanent errors — hard-fail immediately.
    if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("invalid username or password")
        || lower.contains("permission denied")
        || lower.contains("rejected")
        || lower.contains("non-fast-forward")
        || lower.contains("[remote rejected]")
    {
        return false;
    }
    // Transient network / infrastructure signals.
    lower.contains("could not resolve host")
        || lower.contains("unable to connect")
        || lower.contains("connection timed out")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("the remote end hung up")
        || lower.contains("curl error")
        || lower.contains("ssh: connect to host")
        || lower.contains("broken pipe")
        || lower.contains("temporary failure")
        || lower.contains("service unavailable")
        || lower.contains("503")
}

async fn run_git_phase_commit(workspace: &PathBuf, branch: &str, commit_msg: &str) -> Result<String, PhaseCommitError> {
    let ws = workspace.to_str().unwrap_or(".");

    // Apply CIFS-safe git configuration before any index-touching operation.
    //
    // The workspace repo lives on a CIFS/SMB2-backed AccFS share.  Without
    // these settings git's defaults cause two classes of failure on CIFS:
    //
    //   • core.trustctime=false   — CIFS ctime is unreliable; the default
    //     (true) causes git to re-stat every tracked file on every status /
    //     checkout, amplifying SMB2 round-trips and the window for D-state.
    //
    //   • core.preloadIndex=false — The default parallel stat storm across the
    //     SMB2 connection saturates the mount under load and increases the
    //     probability of a stall during index writes.
    //
    //   • gc.auto=0               — The default (6700 loose objects) triggers
    //     background git-gc which writes large pack files to the CIFS share;
    //     on a near-full filesystem this reliably produces D-state hangs and
    //     can itself fill the remaining space.
    //
    //   • index.threads=1         — Serialises index I/O; safer on CIFS where
    //     concurrent index readers can collide over the network lock.
    //
    // These are the same settings applied by scripts/phase-commit.sh (§4),
    // deploy/task-workspace-finalize.sh (§E), and deploy/acc-repo-sync.sh.
    // Root-cause analysis: docs/git-index-write-failure-investigation.md
    //   (Incident 7 — CIFS D-state hang, 2026-04-26).
    //
    // Each `git config` call is best-effort: a failure here (e.g. the .git
    // directory is read-only or the config file is locked) must not abort the
    // commit — the default git behaviour is slower but still correct.
    let cifs_configs: &[(&str, &str)] = &[
        ("core.trustctime",        "false"),
        ("core.checkStat",         "minimal"),
        ("core.preloadIndex",      "false"),
        ("index.threads",          "1"),
        ("gc.auto",                "0"),
        ("fetch.writeCommitGraph", "false"),
    ];
    for (key, value) in cifs_configs {
        let _ = Command::new("git")
            .args(["-C", ws, "config", "--local", key, value])
            .output()
            .await;
    }

    let checkout = Command::new("git")
        .args(["-C", ws, "checkout", "-B", branch])
        .output().await
        .map_err(|e| PhaseCommitError::Hard(format!("git checkout: {e}")))?;
    if !checkout.status.success() {
        return Err(PhaseCommitError::Hard(String::from_utf8_lossy(&checkout.stderr).to_string()));
    }

    let add = Command::new("git")
        .args(["-C", ws, "add", "-A"])
        .output().await
        .map_err(|e| PhaseCommitError::Hard(format!("git add: {e}")))?;
    if !add.status.success() {
        return Err(PhaseCommitError::Hard(String::from_utf8_lossy(&add.stderr).to_string()));
    }

    let commit = Command::new("git")
        .args(["-C", ws, "commit", "-m", commit_msg])
        .output().await
        .map_err(|e| PhaseCommitError::Hard(format!("git commit: {e}")))?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr).to_string();
        if !stderr.contains("nothing to commit") {
            return Err(PhaseCommitError::Hard(stderr));
        }
    }

    // Fetch + rebase onto the remote branch before pushing so that if
    // another agent instance has independently advanced origin/<branch>
    // (e.g. concurrent phase-commit runs) we produce a fast-forward push
    // rather than a non-fast-forward rejection.  Both steps are best-
    // effort: a network failure here is not fatal — the push retry loop
    // below will surface the non-fast-forward error on its first attempt
    // and the next phase-commit cycle will try again.
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new("git")
            .args(["-C", ws, "fetch", "origin", branch])
            .output(),
    ).await;
    let rebase = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new("git")
            .args(["-C", ws, "rebase", &format!("origin/{branch}")])
            .output(),
    ).await;
    // If the rebase itself fails (e.g. genuine conflict), abort it so
    // the working tree is clean and the push will surface the real error.
    if let Ok(Ok(rb)) = rebase {
        if !rb.status.success() {
            let _ = Command::new("git")
                .args(["-C", ws, "rebase", "--abort"])
                .output()
                .await;
        }
    }

    // Retry loop: up to 3 attempts with exponential backoff (10 s, 20 s).
    // Each attempt has its own 120 s timeout so one hung TCP connection
    // cannot consume the full budget.  Permanent errors (auth, non-fast-
    // forward) are surfaced immediately without waiting for the remaining
    // retries.
    const MAX_ATTEMPTS: u32 = 3;
    const PUSH_TIMEOUT_SECS: u64 = 120;
    const BACKOFF_SECS: [u64; 2] = [10, 20];

    let mut last_err = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let push_result = tokio::time::timeout(
            Duration::from_secs(PUSH_TIMEOUT_SECS),
            Command::new("git")
                .args(["-C", ws, "push", "--set-upstream", "origin", branch])
                .output(),
        ).await;

        match push_result {
            Err(_) => {
                // The per-attempt timeout fired — treat as transient.
                last_err = format!("git push timed out after {PUSH_TIMEOUT_SECS}s (attempt {attempt}/{MAX_ATTEMPTS})");
            }
            Ok(Err(e)) => {
                // OS-level spawn/IO error — unlikely to be transient, but
                // surface the full message so the caller can decide.
                return Err(PhaseCommitError::Hard(format!("git push: {e}")));
            }
            Ok(Ok(output)) => {
                if output.status.success() {
                    return Ok(String::from_utf8_lossy(&output.stdout).to_string());
                }
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                // Hard-fail immediately on permanent errors.
                if !is_transient_network_error(&stderr) {
                    return Err(PhaseCommitError::Hard(stderr));
                }
                last_err = format!(
                    "git push failed (attempt {attempt}/{MAX_ATTEMPTS}): {}",
                    stderr.trim()
                );
            }
        }

        // Wait before the next attempt (no sleep after the last attempt).
        if attempt < MAX_ATTEMPTS {
            let wait = BACKOFF_SECS[(attempt as usize) - 1];
            sleep(Duration::from_secs(wait)).await;
        }
    }

    // All retry attempts were transient failures — signal Transient so the
    // caller silently requeues without filing an investigation task.
    Err(PhaseCommitError::Transient(last_err))
}

// ── Shared helpers ────────────────────────────────────────────────────────────

async fn resolve_workspace(cfg: &Config, client: &Client, project_id: &str, task_id: &str) -> PathBuf {
    let shared = cfg.acc_dir.join("shared");

    // AgentFS is mounted at $ACC_DIR/shared (CIFS to the hub's
    // /srv/accfs). Each project lives at <shared>/<slug>, where <slug>
    // matches the server's Project.slug field. Until 2026-04-25 we used
    // <shared>/<project_id> here, which produced an empty stub directory
    // — the actual content lives at <shared>/<slug>. agents ran with
    // empty cwds and "completed" tasks without doing real work.
    //
    // Look up the slug AND the server-recorded agentfs_path; fall back to
    // project_id if the lookup fails so we degrade rather than break.
    let workspace_name: String;
    // agentfs_path is the canonical path recorded by the server at project
    // creation time (e.g. /srv/accfs/shared/<slug>). Consulting it here
    // fixes the case where a phase_commit task was filed on one machine
    // (e.g. macOS, HOME=/Users/jkh) but is executed by an agent on a
    // different machine (Linux) — the locally-derived path does not exist
    // on the executing machine, but the server-side canonical path does.
    let mut server_agentfs_path: Option<String> = None;
    if project_id.is_empty() {
        workspace_name = "default".to_string();
    } else {
        match client.projects().get(project_id).await {
            Ok(p) => {
                server_agentfs_path = p.agentfs_path.clone()
                    .filter(|s| !s.is_empty());
                workspace_name = p.slug
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| project_id.to_string());
            }
            _ => {
                workspace_name = project_id.to_string();
            }
        }
    };

    // 1. Local AgentFS mount: $ACC_DIR/shared/<slug>
    if shared.exists() {
        let p = shared.join(&workspace_name);
        if p.exists() {
            return p;
        }
    }

    // 2. Server-recorded agentfs_path (e.g. /srv/accfs/shared/<slug>).
    //    This is the canonical path regardless of which machine or OS the
    //    agent happens to be running on, fixing the
    //    "cannot change to '/Users/jkh/.acc/shared/acc'" class of failure
    //    where the locally-derived path is OS-specific and nonexistent.
    if let Some(ref ap) = server_agentfs_path {
        let ap_path = std::path::Path::new(ap);
        if ap_path.exists() {
            return ap_path.to_path_buf();
        }
    }

    // 3. Hub-resident fallback: on the AgentFS-server node (do-host1) the
    //    share lives at /srv/accfs/shared/<slug>/ and isn't mounted back
    //    onto ~/.acc/shared/ (no self-loop). Without setup-node.sh's
    //    symlink farm in place, fall back to the canonical hub path so
    //    agents on the hub host still find populated workspaces.
    let hub_path = std::path::Path::new("/srv/accfs/shared").join(&workspace_name);
    if hub_path.exists() {
        return hub_path;
    }

    // 4. Last resort: prefer the server-recorded path even if it doesn't
    //    yet exist (git will create it), then fall back to local derived
    //    paths. This avoids constructing an OS-specific path (e.g.
    //    /Users/jkh/...) that will never exist on a Linux agent.
    if let Some(ap) = server_agentfs_path {
        tracing::warn!(
            project_id = %project_id,
            path = %ap,
            "resolve_workspace: using server-recorded agentfs_path as last-resort \
             fallback but the path does not exist on this machine — git operations \
             may fail if the directory is never created"
        );
        return std::path::PathBuf::from(ap);
    }

    // Fallback: shared/<slug-or-id> (will be created by caller); for
    // task-scoped paths a per-task local dir is the last resort.
    if task_id.is_empty() {
        cfg.acc_dir.join("shared").join(&workspace_name)
    } else {
        cfg.acc_dir.join("task-workspaces").join(task_id)
    }
}

async fn complete_task(cfg: &Config, client: &Client, task_id: &str, output: &str) {
    let truncated = &output[..output.len().min(4096)];
    let _ = client
        .tasks()
        .complete(task_id, Some(&cfg.agent_name), Some(truncated))
        .await;
}

async fn unclaim_task(cfg: &Config, client: &Client, task_id: &str) {
    let _ = client.tasks().unclaim(task_id, Some(&cfg.agent_name)).await;
}

fn is_quenched(cfg: &Config) -> bool {
    cfg.quench_file().exists()
}

fn log(cfg: &Config, msg: &str) {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let line = format!("[{ts}] [tasks] [{}] {msg}", cfg.agent_name);
    eprintln!("{line}");
    let log_path = cfg.log_file("tasks");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
    // CCC-u3c: also emit through tracing so journald (when available)
    // sees this for the consolidated dashboard log viewer.
    tracing::info!(component = "tasks", agent = %cfg.agent_name, "{msg}");
}

fn parse_task_type(s: &str) -> Option<TaskType> {
    use std::str::FromStr;
    TaskType::from_str(s).ok()
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_mock::{HubMock, HubState};
    use serde_json::json;

    fn test_cfg(url: &str) -> Config {
        Config {
            acc_dir: std::path::PathBuf::from("/tmp"),
            acc_url: url.to_string(),
            acc_token: "test-token".to_string(),
            agent_name: "test-agent".to_string(),
            agentbus_token: String::new(),
            pair_programming: true,
            host: "test-host.local".to_string(),
            ssh_user: "testuser".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: 22,
        }
    }

    fn test_client(url: &str) -> Client {
        Client::new(url, "test-token").expect("build client")
    }

    // ── fetch_open_tasks ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_fetch_open_tasks_parses_tasks() {
        let mock = HubMock::with_tasks(vec![
            json!({"id": "t-1", "title": "Alpha", "status": "open", "task_type": "work"}),
            json!({"id": "t-2", "title": "Beta",  "status": "open", "task_type": "work"}),
        ]).await;
        let client = test_client(&mock.url);
        let tasks = fetch_open_tasks(&test_cfg(&mock.url), &client, 10, "work").await.unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0]["id"], "t-1");
    }

    #[tokio::test]
    async fn test_fetch_open_tasks_empty_hub() {
        let mock = HubMock::new().await;
        let client = test_client(&mock.url);
        let tasks = fetch_open_tasks(&test_cfg(&mock.url), &client, 10, "work").await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_fetch_open_tasks_only_open_status() {
        let mock = HubMock::with_tasks(vec![
            json!({"id": "open-1",   "status": "open",    "task_type": "work"}),
            json!({"id": "claimed-1","status": "claimed", "task_type": "work"}),
        ]).await;
        let client = test_client(&mock.url);
        let tasks = fetch_open_tasks(&test_cfg(&mock.url), &client, 10, "work").await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["id"], "open-1");
    }

    #[tokio::test]
    async fn test_fetch_open_tasks_filters_by_task_type() {
        let mock = HubMock::with_tasks(vec![
            json!({"id": "w-1", "status": "open", "task_type": "work"}),
            json!({"id": "r-1", "status": "open", "task_type": "review"}),
            json!({"id": "p-1", "status": "open", "task_type": "phase_commit"}),
        ]).await;
        let client = test_client(&mock.url);
        let work = fetch_open_tasks(&test_cfg(&mock.url), &client, 10, "work").await.unwrap();
        assert_eq!(work.len(), 1);
        assert_eq!(work[0]["id"], "w-1");

        let review = fetch_open_tasks(&test_cfg(&mock.url), &client, 10, "review").await.unwrap();
        assert_eq!(review.len(), 1);
        assert_eq!(review[0]["id"], "r-1");
    }

    #[tokio::test]
    async fn test_fetch_open_tasks_hub_unreachable() {
        let cfg = test_cfg("http://127.0.0.1:1");
        let client = test_client(&cfg.acc_url);
        let result = fetch_open_tasks(&cfg, &client, 5, "work").await;
        assert!(result.is_err(), "unreachable hub must return Err");
    }

    // ── count_active_tasks ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_count_active_tasks_returns_claimed_count() {
        let mock = HubMock::with_state(HubState {
            tasks: vec![
                json!({"id": "c1", "status": "claimed"}),
                json!({"id": "c2", "status": "claimed"}),
                json!({"id": "o1", "status": "open"}),
            ],
            ..Default::default()
        }).await;
        let client = test_client(&mock.url);
        let count = count_active_tasks(&test_cfg(&mock.url), &client).await;
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_count_active_tasks_zero_when_none_claimed() {
        let mock = HubMock::with_tasks(vec![
            json!({"id": "o1", "status": "open"}),
        ]).await;
        let client = test_client(&mock.url);
        let count = count_active_tasks(&test_cfg(&mock.url), &client).await;
        assert_eq!(count, 0);
    }

    // ── claim_task ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_claim_task_success_returns_task() {
        let mock = HubMock::new().await;
        let client = test_client(&mock.url);
        let result = claim_task(&test_cfg(&mock.url), &client, "task-xyz").await;
        assert!(result.is_ok(), "200 → Ok");
        assert_eq!(result.unwrap()["id"], "task-xyz");
    }

    #[tokio::test]
    async fn test_claim_task_conflict_returns_err_409() {
        let mock = HubMock::with_state(HubState { task_claim_status: 409, ..Default::default() }).await;
        let client = test_client(&mock.url);
        let result = claim_task(&test_cfg(&mock.url), &client, "task-abc").await;
        assert!(matches!(result, Err(409)), "409 → Err(409)");
    }

    #[tokio::test]
    async fn test_claim_task_rate_limited_returns_err_429() {
        let mock = HubMock::with_state(HubState { task_claim_status: 429, ..Default::default() }).await;
        let client = test_client(&mock.url);
        let result = claim_task(&test_cfg(&mock.url), &client, "task-def").await;
        assert!(matches!(result, Err(429)), "429 → Err(429)");
    }

    #[tokio::test]
    async fn test_claim_task_blocked_returns_err_423() {
        let mock = HubMock::with_state(HubState { task_claim_status: 423, ..Default::default() }).await;
        let client = test_client(&mock.url);
        let result = claim_task(&test_cfg(&mock.url), &client, "task-blocked").await;
        assert!(matches!(result, Err(423)), "423 → Err(423)");
    }

    // ── parse_review_output ───────────────────────────────────────────────────

    #[test]
    fn test_parse_review_output_approved() {
        let output = r#"{"verdict":"approved","reason":"looks good","gaps":[]}"#;
        let (v, r, g) = parse_review_output(output);
        assert_eq!(v, "approved");
        assert_eq!(r, "looks good");
        assert!(g.is_empty());
    }

    #[test]
    fn test_parse_review_output_approved_with_preamble() {
        let output = r#"Here is my review:

{"verdict":"approved","reason":"well done","gaps":[{"title":"Add tests","description":"Missing unit tests","priority":2}]}"#;
        let (v, r, g) = parse_review_output(output);
        assert_eq!(v, "approved");
        assert_eq!(r, "well done");
        assert_eq!(g.len(), 1);
        assert_eq!(g[0]["title"], "Add tests");
    }

    #[test]
    fn test_parse_review_output_rejected() {
        let output = r#"{"verdict":"rejected","reason":"build is broken","gaps":[{"title":"Fix CI","description":"pipeline fails","priority":0}]}"#;
        let (v, r, g) = parse_review_output(output);
        assert_eq!(v, "rejected");
        assert_eq!(r, "build is broken");
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn test_parse_review_output_unparseable_treated_as_rejected() {
        let output = "This is not JSON at all";
        let (v, r, _) = parse_review_output(output);
        assert_eq!(v, "rejected");
        assert_eq!(r, "unparseable output");
    }

    #[test]
    fn test_parse_review_output_empty_treated_as_rejected() {
        let (v, r, _) = parse_review_output("");
        assert_eq!(v, "rejected");
        assert_eq!(r, "unparseable output");
    }

    // ── submit_for_review ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_submit_for_review_picks_non_self_peer() {
        let mock = HubMock::new().await;
        let client = test_client(&mock.url);
        let cfg = Config {
            agent_name: "boris".to_string(),
            pair_programming: true,
            ..test_cfg(&mock.url)
        };
        let task = json!({"id":"t-1","project_id":"proj","title":"Do work","priority":2});
        let peers = vec!["natasha".to_string(), "boris".to_string()];

        submit_for_review(&cfg, &client, &task, "output here", &peers).await;

        let created = mock.state.read().await.created_tasks.lock().await.clone();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0]["task_type"], "review");
        assert_eq!(created[0]["review_of"], "t-1");
        assert_eq!(created[0]["metadata"]["preferred_executor"], "natasha");
    }

    #[tokio::test]
    async fn test_submit_for_review_no_peers_no_preferred() {
        let mock = HubMock::new().await;
        let client = test_client(&mock.url);
        let cfg = Config {
            agent_name: "natasha".to_string(),
            pair_programming: true,
            ..test_cfg(&mock.url)
        };
        let task = json!({"id":"t-2","project_id":"proj","title":"Solo work","priority":2});

        submit_for_review(&cfg, &client, &task, "done", &[]).await;

        let created = mock.state.read().await.created_tasks.lock().await.clone();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0]["task_type"], "review");
        // No preferred_executor when no peers
        assert!(created[0]["metadata"]["preferred_executor"].is_null() ||
                created[0]["metadata"].get("preferred_executor").is_none());
    }

    #[tokio::test]
    async fn test_submit_for_review_self_only_peer_no_preferred() {
        let mock = HubMock::new().await;
        let client = test_client(&mock.url);
        let cfg = Config {
            agent_name: "natasha".to_string(),
            pair_programming: true,
            ..test_cfg(&mock.url)
        };
        let task = json!({"id":"t-3","project_id":"proj","title":"Solo work","priority":2});
        let peers = vec!["natasha".to_string()]; // only self

        submit_for_review(&cfg, &client, &task, "done", &peers).await;

        let created = mock.state.read().await.created_tasks.lock().await.clone();
        assert_eq!(created.len(), 1);
        // No other peer available, preferred_executor should be absent or empty
        let pref = created[0]["metadata"]["preferred_executor"].as_str().unwrap_or("");
        assert!(pref.is_empty());
    }

    // ── is_transient_network_error ────────────────────────────────────────────
    //
    // Table-driven tests for the pure classification function.
    // Each entry is (description, stderr_string, expected_bool).
    //
    // The function guards two distinct outcomes in the push-retry loop:
    //   true  → silently requeue (no investigation task filed)
    //   false → PhaseCommitError::Hard → investigation task filed
    //
    // Tests cover:
    //   1. Each individual transient signal returns true.
    //   2. Each individual hard signal returns false.
    //   3. Mixed stderr: a hard keyword beats a transient keyword ("rejected"
    //      wins over "broken pipe" — guards the priority-guard block).
    //   4. Edge cases: empty string, whitespace-only, case-insensitivity,
    //      numeric token "503" embedded in longer text.

    #[test]
    fn test_is_transient_network_error_empty_string() {
        // No keywords at all → neither hard nor transient → false.
        assert!(!is_transient_network_error(""));
    }

    #[test]
    fn test_is_transient_network_error_whitespace_only() {
        assert!(!is_transient_network_error("   \t\n  "));
    }

    #[test]
    fn test_is_transient_network_error_unrelated_message() {
        // A failure message that contains none of the recognised keywords.
        assert!(!is_transient_network_error(
            "error: src refspec refs/heads/feature does not match any"
        ));
    }

    // ── Individual transient signals ──────────────────────────────────────────

    #[test]
    fn test_is_transient_could_not_resolve_host() {
        assert!(is_transient_network_error(
            "fatal: unable to access 'https://github.com/org/repo.git/': \
             Could not resolve host: github.com"
        ));
    }

    #[test]
    fn test_is_transient_unable_to_connect() {
        assert!(is_transient_network_error(
            "fatal: unable to connect to github.com"
        ));
    }

    #[test]
    fn test_is_transient_connection_timed_out() {
        assert!(is_transient_network_error(
            "fatal: unable to access '...': Connection timed out"
        ));
    }

    #[test]
    fn test_is_transient_connection_refused() {
        assert!(is_transient_network_error(
            "fatal: unable to access '...': Connection refused"
        ));
    }

    #[test]
    fn test_is_transient_network_is_unreachable() {
        assert!(is_transient_network_error(
            "fatal: unable to access '...': Network is unreachable"
        ));
    }

    #[test]
    fn test_is_transient_remote_end_hung_up() {
        assert!(is_transient_network_error(
            "fatal: the remote end hung up unexpectedly"
        ));
    }

    #[test]
    fn test_is_transient_curl_error() {
        assert!(is_transient_network_error(
            "error: RPC failed; curl error 56: Recv failure: Connection reset by peer"
        ));
    }

    #[test]
    fn test_is_transient_ssh_connect_to_host() {
        assert!(is_transient_network_error(
            "ssh: connect to host github.com port 22: Connection refused"
        ));
    }

    #[test]
    fn test_is_transient_broken_pipe() {
        assert!(is_transient_network_error(
            "error: write error: Broken pipe"
        ));
    }

    #[test]
    fn test_is_transient_temporary_failure() {
        assert!(is_transient_network_error(
            "fatal: unable to access '...': Temporary failure in name resolution"
        ));
    }

    #[test]
    fn test_is_transient_service_unavailable() {
        assert!(is_transient_network_error(
            "error: RPC failed; HTTP 503 Service Unavailable"
        ));
    }

    #[test]
    fn test_is_transient_503_bare_token() {
        // "503" appears as a standalone numeric token (not part of e.g. "15030").
        assert!(is_transient_network_error(
            "remote: 503 Service temporarily unavailable"
        ));
    }

    #[test]
    fn test_is_transient_503_embedded_in_longer_text() {
        // substring match — "503" anywhere in the string is enough.
        assert!(is_transient_network_error(
            "error: The remote server returned HTTP/1.1 503 after 3 retries"
        ));
    }

    // ── Individual hard (permanent) signals ───────────────────────────────────

    #[test]
    fn test_is_hard_authentication_failed() {
        assert!(!is_transient_network_error(
            "remote: Invalid username or password.\n\
             fatal: Authentication failed for 'https://github.com/org/repo.git/'"
        ));
    }

    #[test]
    fn test_is_hard_could_not_read_username() {
        assert!(!is_transient_network_error(
            "fatal: could not read Username for 'https://github.com': \
             terminal prompts disabled"
        ));
    }

    #[test]
    fn test_is_hard_invalid_username_or_password() {
        assert!(!is_transient_network_error(
            "remote: Invalid username or password."
        ));
    }

    #[test]
    fn test_is_hard_permission_denied() {
        assert!(!is_transient_network_error(
            "ERROR: Permission to org/repo.git denied to deploy-key."
        ));
    }

    #[test]
    fn test_is_hard_rejected_non_fast_forward() {
        // Classic non-fast-forward push rejection from GitHub.
        assert!(!is_transient_network_error(
            "To https://github.com/org/repo.git\n\
             ! [rejected]        main -> main (non-fast-forward)\n\
             error: failed to push some refs to 'https://github.com/org/repo.git'\n\
             hint: Updates were rejected because the tip of your current branch is behind"
        ));
    }

    #[test]
    fn test_is_hard_rejected_keyword_alone() {
        // The word "rejected" anywhere → hard failure.
        assert!(!is_transient_network_error("! [rejected] phase/1 (fetch first)"));
    }

    #[test]
    fn test_is_hard_non_fast_forward_keyword_alone() {
        assert!(!is_transient_network_error(
            "Updates were rejected because the remote contains work that you do not have locally.\n\
             hint: non-fast-forward"
        ));
    }

    #[test]
    fn test_is_hard_remote_rejected_bracket() {
        assert!(!is_transient_network_error(
            "! [remote rejected] refs/heads/main -> refs/heads/main (pre-receive hook declined)"
        ));
    }

    // ── Case-insensitivity ────────────────────────────────────────────────────

    #[test]
    fn test_is_transient_case_insensitive_broken_pipe_uppercase() {
        // The implementation lowercases before matching; verify this.
        assert!(is_transient_network_error("BROKEN PIPE occurred during push"));
    }

    #[test]
    fn test_is_hard_case_insensitive_rejected_uppercase() {
        assert!(!is_transient_network_error("! [REJECTED] main -> main (fetch first)"));
    }

    #[test]
    fn test_is_hard_case_insensitive_permission_denied_mixed_case() {
        assert!(!is_transient_network_error("Permission Denied (publickey)."));
    }

    // ── Priority guard: hard keyword beats transient keyword ──────────────────
    //
    // The hard-keyword block is an early-return guard that runs BEFORE the
    // transient-keyword check. A single stderr that contains both a hard
    // signal and a transient signal must be classified as hard (false), not
    // transient (true). This is the most important regression boundary: if the
    // order of checks were accidentally reversed, a "rejected" push would be
    // silently requeued instead of triggering an investigation task.

    #[test]
    fn test_is_hard_rejected_beats_broken_pipe() {
        // "rejected" (hard) + "broken pipe" (transient) in same stderr → hard.
        let stderr = "error: write error: Broken pipe\n\
                      ! [rejected] main -> main (non-fast-forward)";
        assert!(
            !is_transient_network_error(stderr),
            "'rejected' in stderr must classify as hard even when 'broken pipe' is also present"
        );
    }

    #[test]
    fn test_is_hard_authentication_failed_beats_connection_timed_out() {
        let stderr = "fatal: unable to access '...': Connection timed out\n\
                      fatal: Authentication failed for 'https://github.com/org/repo.git/'";
        assert!(
            !is_transient_network_error(stderr),
            "'authentication failed' must win over 'connection timed out'"
        );
    }

    #[test]
    fn test_is_hard_permission_denied_beats_service_unavailable() {
        // The hard-keyword check looks for the literal substring "permission denied".
        // Use a message where that substring is present verbatim.
        let stderr = "remote: HTTP 503 Service Unavailable\n\
                      Permission denied (publickey).";
        assert!(
            !is_transient_network_error(stderr),
            "'permission denied' must win over '503 service unavailable'"
        );
    }

    #[test]
    fn test_is_hard_remote_rejected_beats_curl_error() {
        let stderr = "error: RPC failed; curl error 56 Recv failure\n\
                      ! [remote rejected] main (hook declined)";
        assert!(
            !is_transient_network_error(stderr),
            "'[remote rejected]' must win over 'curl error'"
        );
    }

    // ── PhaseCommitError variant routing ──────────────────────────────────────
    //
    // Verify that the two variants have the expected Display output and that
    // the Debug representation is present (used in log lines).
    //
    // These are deliberately simple: the routing *logic* (which variant is
    // chosen) lives in run_git_phase_commit which requires a live git repo and
    // async runtime. The tests here guard the data-carrying contract of the
    // enum so that a refactor (e.g. adding a third variant) does not silently
    // change the formatted log messages that operators read.

    #[test]
    fn test_phase_commit_error_transient_display() {
        let e = PhaseCommitError::Transient("git push timed out after 120s (attempt 3/3)".to_string());
        let s = e.to_string();
        assert!(
            s.starts_with("(transient) "),
            "Transient display must start with '(transient) ', got: {s:?}"
        );
        assert!(
            s.contains("git push timed out"),
            "Transient display must include the inner message, got: {s:?}"
        );
    }

    #[test]
    fn test_phase_commit_error_hard_display() {
        let e = PhaseCommitError::Hard("! [rejected] main -> main (non-fast-forward)".to_string());
        let s = e.to_string();
        // Hard errors are displayed verbatim with no prefix.
        assert!(
            !s.contains("(transient)"),
            "Hard display must NOT contain '(transient)', got: {s:?}"
        );
        assert!(
            s.contains("[rejected]"),
            "Hard display must include the inner message, got: {s:?}"
        );
    }

    #[test]
    fn test_phase_commit_error_debug_both_variants() {
        // Ensure Debug is derived (used in unwrap/expect messages and tracing).
        let t = PhaseCommitError::Transient("flaky".to_string());
        let h = PhaseCommitError::Hard("auth".to_string());
        // Debug formatting should not panic and should include the variant name.
        let td = format!("{t:?}");
        let hd = format!("{h:?}");
        assert!(td.contains("Transient"), "Debug for Transient should mention variant name");
        assert!(hd.contains("Hard"), "Debug for Hard should mention variant name");
    }

    #[test]
    fn test_phase_commit_error_transient_empty_message() {
        // Edge case: empty message should still format correctly.
        let e = PhaseCommitError::Transient(String::new());
        assert_eq!(e.to_string(), "(transient) ");
    }

    #[test]
    fn test_phase_commit_error_hard_empty_message() {
        let e = PhaseCommitError::Hard(String::new());
        assert_eq!(e.to_string(), "");
    }

    // ── resolve_workspace ─────────────────────────────────────────────────────
    //
    // Priority order under test:
    //   1. local mount   — $ACC_DIR/shared/<slug>  exists on this machine
    //   2. agentfs_path  — server-recorded path     exists on this machine
    //   3. hub path      — /srv/accfs/shared/<slug> exists on this machine
    //   4. agentfs_path (non-existent) — returned as last-resort over local-derived path
    //   5. local derived — $ACC_DIR/shared/<slug>   (no server-side path known)

    /// Helper: build a `HubMock` with a single project record.
    async fn mock_with_project(
        id: &str,
        slug: Option<&str>,
        agentfs_path: Option<&str>,
    ) -> HubMock {
        use crate::hub_mock::{HubState, MockProject};
        let mut projects = std::collections::HashMap::new();
        projects.insert(
            id.to_string(),
            MockProject {
                id: id.to_string(),
                name: id.to_string(),
                slug: slug.map(str::to_string),
                agentfs_path: agentfs_path.map(str::to_string),
            },
        );
        HubMock::with_state(HubState {
            projects,
            ..Default::default()
        })
        .await
    }

    // ── Branch 1: local AgentFS mount exists ─────────────────────────────────

    /// When `$ACC_DIR/shared/<slug>` already exists on disk, resolve_workspace
    /// must return that local path immediately (highest priority).
    #[tokio::test]
    async fn test_resolve_workspace_local_mount_wins() {
        let dir = tempfile::tempdir().unwrap();
        let slug = "my-project";
        let local_ws = dir.path().join("shared").join(slug);
        std::fs::create_dir_all(&local_ws).unwrap();

        let mock = mock_with_project("proj-1", Some(slug), Some("/srv/accfs/shared/my-project")).await;
        let cfg = Config {
            acc_dir: dir.path().to_path_buf(),
            ..test_cfg(&mock.url)
        };
        let client = test_client(&mock.url);

        let result = resolve_workspace(&cfg, &client, "proj-1", "").await;
        assert_eq!(result, local_ws, "local mount should win");
    }

    // ── Branch 2: server agentfs_path exists on this machine ─────────────────

    /// Cross-OS scenario: the project was filed on macOS with a macOS-derived
    /// local path, but the executing agent is Linux.  The local mount
    /// (`$ACC_DIR/shared/<slug>`) does NOT exist, but the server-recorded
    /// `agentfs_path` points to a real directory on this machine (simulating
    /// the AgentFS NFS/CIFS share mounted at the same canonical path on both
    /// OSes, e.g. `/srv/accfs/shared/<slug>`).
    ///
    /// resolve_workspace must prefer the server-recorded path over the
    /// locally-derived fallback.
    #[tokio::test]
    async fn test_resolve_workspace_server_agentfs_path_exists_wins_over_local_derived() {
        let dir = tempfile::tempdir().unwrap();
        // Intentionally do NOT create $ACC_DIR/shared/<slug> — simulating the
        // "Linux agent receiving a macOS-origin path" scenario where the slug
        // subdirectory under the local share does not exist.

        // Create a separate temp dir that acts as the canonical server-side path.
        let server_dir = tempfile::tempdir().unwrap();
        let server_path = server_dir.path().to_str().unwrap().to_string();

        let slug = "cross-os-project";
        let mock = mock_with_project("proj-xos", Some(slug), Some(&server_path)).await;
        let cfg = Config {
            acc_dir: dir.path().to_path_buf(),
            ..test_cfg(&mock.url)
        };
        let client = test_client(&mock.url);

        let result = resolve_workspace(&cfg, &client, "proj-xos", "").await;
        assert_eq!(
            result,
            std::path::PathBuf::from(&server_path),
            "server-recorded agentfs_path that exists on this machine should win \
             over the locally-derived path when the local mount is absent"
        );
        // Confirm the local-derived path was NOT returned.
        assert_ne!(result, dir.path().join("shared").join(slug));
    }

    // ── Branch 3: /srv/accfs/shared/<slug> fallback (skipped unless present) ─

    /// When neither the local mount nor the server agentfs_path exists, but
    /// the hub-resident path `/srv/accfs/shared/<slug>` exists, it should win.
    /// We only run this branch if the directory is actually creatable (i.e. on
    /// the hub host itself); otherwise we verify the function returns a
    /// non-panicking path at all.
    #[tokio::test]
    async fn test_resolve_workspace_hub_path_branch_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let slug = "hub-project";
        // agentfs_path points somewhere that doesn't exist (so branch 2 is skipped)
        let mock = mock_with_project("proj-hub", Some(slug), Some("/nonexistent/agentfs/path-xyz")).await;
        let cfg = Config {
            acc_dir: dir.path().to_path_buf(),
            ..test_cfg(&mock.url)
        };
        let client = test_client(&mock.url);

        // Just verify it returns without panicking; the exact path depends on
        // what exists on the test machine.
        let result = resolve_workspace(&cfg, &client, "proj-hub", "").await;
        assert!(!result.as_os_str().is_empty(), "must return a non-empty path");
    }

    // ── Branch 4: agentfs_path non-existent returned as last resort ──────────

    /// When the local mount, the server agentfs_path, and the hub path all
    /// don't exist, resolve_workspace must return the server-recorded
    /// agentfs_path verbatim (so git clone can create it later) rather than
    /// constructing an OS-specific locally-derived path that would never exist
    /// on a foreign machine.
    ///
    /// This is the core regression guard: a Linux agent must NOT fall back to
    /// e.g. `/Users/jkh/.acc/shared/<slug>` when the server says the canonical
    /// workspace is somewhere else entirely.
    #[tokio::test]
    async fn test_resolve_workspace_agentfs_path_nonexistent_preferred_over_local_derived() {
        let dir = tempfile::tempdir().unwrap();
        let slug = "macos-origin-project";
        // Simulate: project was created on macOS; the server recorded an
        // agentfs_path that is NOT under this agent's $ACC_DIR/shared tree
        // and does not actually exist on this Linux agent.
        let foreign_agentfs_path = "/srv/accfs/shared/macos-origin-project-nonexistent-xyz123";

        let mock = mock_with_project("proj-foreign", Some(slug), Some(foreign_agentfs_path)).await;
        let cfg = Config {
            acc_dir: dir.path().to_path_buf(),
            ..test_cfg(&mock.url)
        };
        let client = test_client(&mock.url);

        let result = resolve_workspace(&cfg, &client, "proj-foreign", "").await;

        // The server-recorded path must win over the locally-derived path.
        assert_eq!(
            result,
            std::path::PathBuf::from(foreign_agentfs_path),
            "non-existent server agentfs_path should be returned as last resort, \
             not a locally-derived OS-specific path"
        );
        // Crucially, must NOT return a path under this agent's $ACC_DIR.
        assert!(
            !result.starts_with(dir.path()),
            "result must not be under this agent's acc_dir (that would be the \
             locally-derived path, wrong on a cross-OS hop)"
        );
    }

    // ── Branch 5: local derived path when no server-side info ────────────────

    /// When the project lookup fails (404 or network error), resolve_workspace
    /// falls all the way through to the locally-derived path
    /// `$ACC_DIR/shared/<project_id>` (or the per-task directory when task_id
    /// is non-empty).
    #[tokio::test]
    async fn test_resolve_workspace_local_derived_fallback_on_lookup_failure() {
        let dir = tempfile::tempdir().unwrap();
        // No project registered in the mock → GET /api/projects/unknown → 404
        let mock = HubMock::new().await;
        let cfg = Config {
            acc_dir: dir.path().to_path_buf(),
            ..test_cfg(&mock.url)
        };
        let client = test_client(&mock.url);

        let result = resolve_workspace(&cfg, &client, "unknown-proj", "").await;
        assert_eq!(
            result,
            dir.path().join("shared").join("unknown-proj"),
            "should fall back to acc_dir/shared/<project_id> when lookup fails"
        );
    }

    /// Same as above but with a non-empty task_id: the last-resort path must
    /// be `$ACC_DIR/task-workspaces/<task_id>`.
    #[tokio::test]
    async fn test_resolve_workspace_task_scoped_fallback_on_lookup_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mock = HubMock::new().await;
        let cfg = Config {
            acc_dir: dir.path().to_path_buf(),
            ..test_cfg(&mock.url)
        };
        let client = test_client(&mock.url);

        let result = resolve_workspace(&cfg, &client, "unknown-proj", "task-42").await;
        assert_eq!(
            result,
            dir.path().join("task-workspaces").join("task-42"),
            "with a task_id, fallback should be acc_dir/task-workspaces/<task_id>"
        );
    }
}
