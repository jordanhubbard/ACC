# Conversation Chains

Conversation chains are ACC's durable provenance record for Slack and Telegram interactions.

A chain is not a Slack thread or Telegram chat directly. Those are source bindings. The chain is the cross-platform object that records:

- Stand-alone human and bot messages.
- Reactions to prior messages.
- Long threaded context.
- Tasks requested from the conversation.
- Participants, entities, and channels involved.
- Final status or outcome declarations.

## Storage Model

The server stores chains in SQLite:

- `conversation_chains`: mutable chain summary, source binding, status, outcome, timestamps, metadata.
- `conversation_chain_events`: append-only facts such as messages, reactions, bot replies, errors, task mentions, and status declarations.
- `conversation_chain_participants`: derived index of humans, bots, and agents seen in the chain.
- `conversation_chain_entities`: derived index of tagged projects, services, repos, errors, channels, files, and tasks.
- `conversation_chain_tasks`: tasks spawned from or discussed in the chain, including their latest resolved state.

Raw events are the source of truth. Chain status, title, participants, entities, and task state are derived indexes.

## API

- `POST /api/chains`: create or upsert a chain.
- `GET /api/chains`: list chains with filters such as `source`, `workspace`, `channel_id`, `status`, `participant`, `entity_type`, and `entity_id`.
- `GET /api/chains/:id`: fetch a full chain including events, participants, entities, and linked tasks.
- `PATCH /api/chains/:id`: update derived fields such as `status`, `outcome`, `summary`, and `metadata`.
- `POST /api/chains/:id/events`: append a raw event. Duplicate `source_event_id` values are idempotent per chain.
- `POST /api/chains/:id/tasks`: link a fleet task to a chain.

Tasks may also carry top-level `chain_id` or `source_chain_id` on `POST /api/tasks`; the server stores it in task metadata and automatically links the task.

## Gateway Behavior

The Hermes Slack and Telegram gateways emit chain events before deciding whether to answer. This means non-mention channel comments still become context without forcing the bot to respond.

Slack chains are keyed by workspace, channel, and thread root timestamp. Telegram chains are keyed by chat and thread or reply root message id.

Bot replies, gateway reset commands, reactions, and LLM errors are appended back into the same chain, giving operators a single record of what was asked, who participated, what tasks were created, and whether the conversation resolved.
