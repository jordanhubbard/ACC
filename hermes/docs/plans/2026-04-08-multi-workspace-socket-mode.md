# Multi-Workspace Socket Mode Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Enable hermes-agent Slack adapter to receive events from multiple Slack workspaces simultaneously, each with its own Socket Mode connection.

**Architecture:** Replace the single `AsyncApp` + `AsyncSocketModeHandler` with a per-account loop: for each (bot_token, app_token) pair, spin up an independent `AsyncApp` and `AsyncSocketModeHandler`. All handlers route events into the existing unified `_handle_slack_message()` pipeline. The existing multi-workspace *sending* infrastructure (`_team_clients`, `_get_client()`) is preserved and extended.

**Tech Stack:** Python, slack-bolt (AsyncApp, AsyncSocketModeHandler), slack-sdk (AsyncWebClient), asyncio

---

## Current State

- Single `SLACK_APP_TOKEN` env var → single Socket Mode connection
- Multiple `SLACK_BOT_TOKEN`s (comma-separated or `slack_tokens.json`) → multiple WebClients for sending
- Can *send* to multiple workspaces, can only *receive* from one
- Scoped lock prevents two gateways from using the same app token

## Target State

- Multiple (bot_token, app_token) pairs loaded from `~/.hermes/slack_accounts.json`
- One `AsyncApp` + `AsyncSocketModeHandler` per account
- Each Socket Mode connection receives events for its workspace
- Unified event handling pipeline (existing `_handle_slack_message`)
- Backward compatible: if only env vars set (single workspace), works exactly as before
- Per-app-token scoped locks (one per account, not one global)

## Config Format

New file: `~/.hermes/slack_accounts.json`

```json
[
  {
    "name": "primary-workspace",
    "bot_token": "xoxb-...",
    "app_token": "xapp-..."
  },
  {
    "name": "secondary-workspace",
    "bot_token": "xoxb-...",
    "app_token": "xapp-..."
  }
]
```

Fallback: if this file doesn't exist, fall back to `SLACK_BOT_TOKEN` + `SLACK_APP_TOKEN` env vars (single workspace, backward compat).

---

## Tasks

### Task 1: Add account loading from slack_accounts.json

**Objective:** Load multi-workspace account configs from a new JSON file, with fallback to env vars.

**File:** `gateway/platforms/slack.py`

**Changes:**

Add a helper method `_load_accounts()` that returns a list of `{"name": str, "bot_token": str, "app_token": str}` dicts. Logic:

1. Check `~/.hermes/slack_accounts.json` — if exists and non-empty, load accounts from there
2. Else fall back to `SLACK_BOT_TOKEN` (+ comma-sep + `slack_tokens.json`) paired with single `SLACK_APP_TOKEN`
3. Validate: every account must have both bot_token and app_token
4. Return list of account dicts

### Task 2: Refactor __init__ for multi-handler state

**Objective:** Replace single `_app`/`_handler`/`_socket_mode_task` with per-account collections.

**File:** `gateway/platforms/slack.py`

**Changes to `__init__`:**

```python
# Replace:
self._app: Optional[AsyncApp] = None
self._handler: Optional[AsyncSocketModeHandler] = None
self._socket_mode_task: Optional[asyncio.Task] = None

# With:
self._apps: Dict[str, AsyncApp] = {}                    # account_name → AsyncApp
self._handlers: Dict[str, AsyncSocketModeHandler] = {}   # account_name → handler
self._socket_mode_tasks: Dict[str, asyncio.Task] = {}    # account_name → task
self._app: Optional[AsyncApp] = None                     # primary app (backward compat for send fallback)
```

### Task 3: Refactor connect() to start one Socket Mode per account

**Objective:** Replace single-socket startup with a per-account loop.

**File:** `gateway/platforms/slack.py`

**Changes to `connect()`:**

1. Call `_load_accounts()` to get account list
2. For each account:
   a. Acquire scoped lock for that account's app_token
   b. Create `AsyncApp(token=bot_token)`
   c. `auth_test()` to get team_id, bot_user_id
   d. Register into `_team_clients`, `_team_bot_user_ids`
   e. Register event handlers (message, app_mention, /hermes, approval actions) — all routing to the shared `_handle_slack_message()`
   f. Create `AsyncSocketModeHandler(app, app_token)`
   g. `asyncio.create_task(handler.start_async())`
   h. Store in `_apps`, `_handlers`, `_socket_mode_tasks`
3. First account's app becomes `self._app` (backward compat fallback)
4. Log total accounts + workspaces connected

### Task 4: Refactor disconnect() to close all handlers

**Objective:** Clean shutdown of all Socket Mode connections.

**File:** `gateway/platforms/slack.py`

**Changes to `disconnect()`:**

```python
async def disconnect(self) -> None:
    for name, handler in self._handlers.items():
        try:
            await handler.close_async()
        except Exception as e:
            logger.warning("[Slack] Error closing handler %s: %s", name, e)
    
    # Cancel all socket mode tasks
    for name, task in self._socket_mode_tasks.items():
        if not task.done():
            task.cancel()
    
    self._running = False
    
    # Release all token locks
    try:
        from gateway.status import release_scoped_lock
        for identity in getattr(self, '_token_lock_identities', []):
            release_scoped_lock('slack-app-token', identity)
        self._token_lock_identities = []
    except Exception:
        pass
    
    logger.info("[Slack] Disconnected")
```

### Task 5: Update _get_client and send path

**Objective:** Ensure sending still works correctly — route to correct workspace WebClient.

**File:** `gateway/platforms/slack.py`

**Changes:** Minimal — `_get_client()` already works via `_team_clients[team_id]`. Just verify the fallback `self._app.client` still works when `self._app` is set to the primary account's app.

### Task 6: Update send() and other methods that reference self._app

**Objective:** Audit all uses of `self._app` and ensure they work with multi-account setup.

**File:** `gateway/platforms/slack.py`

Search for all `self._app` references. Most should already work since `_get_client()` handles routing. The main concern is places that use `self._app.client` directly — ensure they go through `_get_client()` instead.

### Task 7: Add tests for multi-account loading and connection

**Objective:** Test the new `_load_accounts()` and multi-handler connect flow.

**File:** `tests/gateway/test_slack.py`

Tests:
- `test_load_accounts_from_file` — reads slack_accounts.json
- `test_load_accounts_fallback_env` — falls back to env vars
- `test_connect_multi_account` — creates multiple handlers
- `test_disconnect_multi_account` — closes all handlers

### Task 8: Commit and push

```bash
git add -A
git commit -m "feat(slack): multi-workspace Socket Mode — one connection per account

Each (bot_token, app_token) pair gets its own AsyncApp and Socket Mode
handler, enabling event reception from multiple Slack workspaces.

Config: ~/.hermes/slack_accounts.json with array of {name, bot_token, app_token}.
Falls back to SLACK_BOT_TOKEN + SLACK_APP_TOKEN env vars for single-workspace."
git push origin feature/multi-workspace-socket-mode
```
