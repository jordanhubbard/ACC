//! LLM-callable tools that expose Slack workspace introspection.
//!
//! Each tool wraps one Slack Web API method and is registered with the
//! `ToolRegistry` only when the gateway has resolved Slack tokens (i.e.,
//! when there is a workspace to talk to). The bot token is captured in
//! a shared `SlackApiClient` so multiple tools share one HTTP client.
//!
//! These tools read the workspace; they do not send messages — message
//! send goes through the gateway's normal post-handling path so audit
//! and threading semantics stay consistent.

use super::slack_api::SlackApiClient;
use super::tool::{Tool, ToolResult};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

fn ok_json(v: Value) -> ToolResult {
    serde_json::to_string(&v).map_err(|e| format!("serialize: {e}"))
}

// ── users.info ────────────────────────────────────────────────────────────────

pub struct SlackUsersInfoTool {
    client: Arc<SlackApiClient>,
}

impl SlackUsersInfoTool {
    pub fn new(client: Arc<SlackApiClient>) -> Self {
        Self { client }
    }
}

impl Tool for SlackUsersInfoTool {
    fn name(&self) -> &str {
        "slack_users_info"
    }
    fn description(&self) -> &str {
        "Look up a Slack user by their user ID (e.g., U02ABCDEF). Returns the \
         full profile: real_name, display_name, email when available, time \
         zone, and avatar URL. Use this whenever you need to identify, \
         address, or learn about a person in the workspace."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "user_id": {
                    "type": "string",
                    "description": "Slack user ID, e.g. U02ABCDEF. Strip any leading @ or <@...>"
                }
            },
            "required": ["user_id"]
        })
    }
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let raw = input["user_id"].as_str().unwrap_or("").trim();
            let user_id = raw
                .trim_start_matches("<@")
                .trim_end_matches('>')
                .trim_start_matches('@');
            if user_id.is_empty() {
                return Err("user_id is required".to_string());
            }
            ok_json(self.client.users_info(user_id).await?)
        })
    }
}

// ── users.list ────────────────────────────────────────────────────────────────

pub struct SlackUsersListTool {
    client: Arc<SlackApiClient>,
}

impl SlackUsersListTool {
    pub fn new(client: Arc<SlackApiClient>) -> Self {
        Self { client }
    }
}

impl Tool for SlackUsersListTool {
    fn name(&self) -> &str {
        "slack_users_list"
    }
    fn description(&self) -> &str {
        "List users in the Slack workspace, paginated. Use limit (max 200) \
         to bound the page. The response includes a response_metadata.next_cursor \
         that you can pass back as cursor to fetch the next page."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "description": "Page size (1-200, default 50)"},
                "cursor": {"type": "string", "description": "Cursor from a prior page (optional)"}
            }
        })
    }
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let limit = input["limit"]
                .as_u64()
                .map(|v| v.clamp(1, 200) as u32)
                .or(Some(50));
            let cursor = input["cursor"].as_str();
            ok_json(self.client.users_list(limit, cursor).await?)
        })
    }
}

// ── conversations.members ─────────────────────────────────────────────────────

pub struct SlackConversationsMembersTool {
    client: Arc<SlackApiClient>,
}

impl SlackConversationsMembersTool {
    pub fn new(client: Arc<SlackApiClient>) -> Self {
        Self { client }
    }
}

impl Tool for SlackConversationsMembersTool {
    fn name(&self) -> &str {
        "slack_conversations_members"
    }
    fn description(&self) -> &str {
        "List the user IDs that are members of a Slack channel. Pass the channel \
         ID (e.g., C0AMNRSN9EZ); resolve user IDs separately with slack_users_info \
         if you need names. Paginated via cursor."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {"type": "string", "description": "Channel ID (C…)"},
                "limit": {"type": "integer", "description": "Page size (1-1000, default 100)"},
                "cursor": {"type": "string", "description": "Cursor from a prior page (optional)"}
            },
            "required": ["channel"]
        })
    }
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let channel = input["channel"].as_str().unwrap_or("").trim();
            if channel.is_empty() {
                return Err("channel is required".to_string());
            }
            let limit = input["limit"]
                .as_u64()
                .map(|v| v.clamp(1, 1000) as u32)
                .or(Some(100));
            let cursor = input["cursor"].as_str();
            ok_json(
                self.client
                    .conversations_members(channel, limit, cursor)
                    .await?,
            )
        })
    }
}

// ── conversations.history ─────────────────────────────────────────────────────

pub struct SlackConversationsHistoryTool {
    client: Arc<SlackApiClient>,
}

impl SlackConversationsHistoryTool {
    pub fn new(client: Arc<SlackApiClient>) -> Self {
        Self { client }
    }
}

impl Tool for SlackConversationsHistoryTool {
    fn name(&self) -> &str {
        "slack_conversations_history"
    }
    fn description(&self) -> &str {
        "Read recent messages from a Slack channel. Pass the channel ID (C…). \
         The response is paginated via cursor and bounded by limit (max 100). \
         Optional oldest is a Slack timestamp (e.g., '1700000000.000000') that \
         restricts results to messages newer than that ts."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {"type": "string", "description": "Channel ID (C…)"},
                "limit": {"type": "integer", "description": "Page size (1-100, default 20)"},
                "cursor": {"type": "string", "description": "Cursor from a prior page (optional)"},
                "oldest": {"type": "string", "description": "Slack timestamp lower bound (optional)"}
            },
            "required": ["channel"]
        })
    }
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let channel = input["channel"].as_str().unwrap_or("").trim();
            if channel.is_empty() {
                return Err("channel is required".to_string());
            }
            let limit = input["limit"]
                .as_u64()
                .map(|v| v.clamp(1, 100) as u32)
                .or(Some(20));
            let cursor = input["cursor"].as_str();
            let oldest = input["oldest"].as_str();
            ok_json(
                self.client
                    .conversations_history(channel, limit, cursor, oldest)
                    .await?,
            )
        })
    }
}

// ── team.info ─────────────────────────────────────────────────────────────────

pub struct SlackTeamInfoTool {
    client: Arc<SlackApiClient>,
}

impl SlackTeamInfoTool {
    pub fn new(client: Arc<SlackApiClient>) -> Self {
        Self { client }
    }
}

impl Tool for SlackTeamInfoTool {
    fn name(&self) -> &str {
        "slack_team_info"
    }
    fn description(&self) -> &str {
        "Return metadata about the current Slack workspace: id, name, domain, \
         and icon. Use to confirm which workspace you are operating in."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }
    fn execute<'a>(
        &'a self,
        _input: Value,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move { ok_json(self.client.team_info().await?) })
    }
}

/// Construct the full set of Slack tools sharing one HTTP client.
pub fn all_slack_tools(client: Arc<SlackApiClient>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(SlackUsersInfoTool::new(client.clone())),
        Box::new(SlackUsersListTool::new(client.clone())),
        Box::new(SlackConversationsMembersTool::new(client.clone())),
        Box::new(SlackConversationsHistoryTool::new(client.clone())),
        Box::new(SlackTeamInfoTool::new(client)),
    ]
}
