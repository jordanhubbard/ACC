---
name: acc-project-tracking
description: Create, query, and update ACC projects to track ongoing work. Use when starting a body of work, checking what's active across the fleet, or linking queue tasks to a project.
version: 1.0.0
platforms: [linux, macos]
metadata:
  hermes:
    tags: [acc, projects, tracking, fleet]
    category: infrastructure
required_environment_variables:
  - name: ACC_URL
    help: "Set in ~/.acc/.env as ACC_URL."
  - name: ACC_AGENT_TOKEN
    help: "Set in ~/.acc/.env as ACC_AGENT_TOKEN."
---

# ACC Project Tracking

Projects are the unit of work visibility across the fleet. When you start a body
of work that spans multiple queue tasks or sessions, create a project first so
the fleet knows what's being worked on and by whom.

## Project schema

| Field | Type | Description |
|---|---|---|
| `id` | string | Auto-assigned (`proj-<ms>`) |
| `name` | string | Human-readable name (required) |
| `slug` | string | URL-safe name derived from `name` |
| `status` | string | `active` \| `blocked` \| `archived` |
| `description` | string | What the project is and why it exists |
| `owner` | string | Agent or person who owns the project |
| `assignee` | string | Agent currently executing the work |
| `notes` | string | Running log of progress, blockers, decisions |
| `tags` | array | Free-form labels |
| `git_url` | string | Repo to clone into agentfs |
| `repoUrl` | string | GitHub URL for display |
| `agentfs_path` | string | Where workspace lives on AgentFS |
| `clone_status` | string | `none` \| `pending` \| `ready` \| `failed` |
| `createdAt` | ISO timestamp | |
| `updatedAt` | ISO timestamp | |

---

## Create a project

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null

curl -sf -X POST "${ACC_URL}/api/projects" \
  -H "Authorization: Bearer $ACC_AGENT_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$(python3 -c "
import json, os
print(json.dumps({
  'name':        'My Project',
  'description': 'What this project does and why',
  'owner':       os.environ.get('AGENT_NAME', ''),
  'assignee':    os.environ.get('AGENT_NAME', ''),
  'status':      'active',
  'tags':        ['fleet', 'infra'],
}))
")"
```

Returns `{"ok": true, "project": {...}}`. Save the `id` field — you'll use it to
update and query.

---

## List active projects

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null

curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" \
  "${ACC_URL}/api/projects?status=active" \
| python3 -c "
import json, sys
data = json.load(sys.stdin)
projects = data.get('projects', [])
if not projects:
    print('No active projects')
else:
    for p in projects:
        print(f\"{p['id']}  {p.get('assignee','?'):12s}  {p['name']}  [{p.get('status','?')}]\")
"
```

Query params:
- `status=active` — filter by status
- `tag=gpu` — filter by tag
- `q=search+term` — substring search on name/slug/description
- `limit=N&offset=N` — pagination

---

## Get a single project

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null
PROJECT_ID="proj-1234567890"   # or the slug

curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" \
  "${ACC_URL}/api/projects/${PROJECT_ID}" | python3 -m json.tool
```

Also works with `owner/repo` slug for GitHub-style projects:
```bash
curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" \
  "${ACC_URL}/api/projects/jordanhubbard/ACC"
```

---

## Update a project

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null
PROJECT_ID="proj-1234567890"

curl -sf -X PATCH "${ACC_URL}/api/projects/${PROJECT_ID}" \
  -H "Authorization: Bearer $ACC_AGENT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"status": "blocked", "notes": "Waiting on GPU availability"}'
```

Patchable fields: `name`, `description`, `status`, `owner`, `assignee`, `notes`,
`tags`, `repoUrl`, `slackChannels`, `git_url`, `slug`, `agentfs_path`, `clone_status`.

---

## Archive a project (soft delete)

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null

curl -sf -X DELETE "${ACC_URL}/api/projects/${PROJECT_ID}" \
  -H "Authorization: Bearer $ACC_AGENT_TOKEN"
```

This sets `status=archived`. The record is kept.

To permanently remove (also deletes the agentfs workspace directory):

```bash
curl -sf -X DELETE "${ACC_URL}/api/projects/${PROJECT_ID}?hard=true" \
  -H "Authorization: Bearer $ACC_AGENT_TOKEN"
```

---

## Fleet-wide work view

```bash
source ~/.acc/.env 2>/dev/null || source ~/.ccc/.env 2>/dev/null

curl -sf -H "Authorization: Bearer $ACC_AGENT_TOKEN" "${ACC_URL}/api/projects" \
| python3 -c "
import json, sys
data = json.load(sys.stdin)
projects = data.get('projects', [])
active = [p for p in projects if p.get('status') == 'active']
print(f'Active projects: {len(active)} / {data.get(\"total\", len(projects))} total')
print()
for p in sorted(active, key=lambda x: x.get('updatedAt', '')):
    owner    = p.get('owner') or p.get('assignee') or '?'
    name     = p['name']
    notes    = p.get('notes', '')
    notes_preview = (notes[:60] + '...') if len(notes) > 60 else notes
    print(f'  {owner:12s}  {name}')
    if notes_preview:
        print(f'               {notes_preview}')
"
```

---

## Rules

- **Create a project before starting multi-session work.** Single queue tasks
  don't need a project; bodies of work that span sessions or agents do.
- **Set `assignee` to `$AGENT_NAME`.** This is how the fleet knows who is doing what.
- **Update `notes` as you make progress.** It's the running log that lets other
  agents and sessions pick up where you left off.
- **Use `status=blocked` when waiting.** Don't leave projects appearing active when
  no work is happening — it misleads the fleet.
- **Archive when done.** Don't leave stale active projects.
