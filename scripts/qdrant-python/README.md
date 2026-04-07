# Qdrant Python Scripts (Hermes Integration)

Zero-dependency Python scripts for Qdrant vector DB integration with Hermes agents.
Uses only stdlib (urllib, json, hashlib) — no pip dependencies required.

All embedding calls route through **tokenhub** (http://127.0.0.1:8090/v1/embeddings),
never directly to NVIDIA NIM or other providers.

## Scripts

- **qdrant_common.py** — Shared utilities: embedding, Qdrant HTTP API, chunking, auth
- **qdrant_ingest.py** — Ingest Hermes sessions + memory into Qdrant
- **qdrant_search.py** — Semantic search across sessions, memories, Slack history

## Usage

```bash
# Ingest new sessions
python3 qdrant_ingest.py

# Full re-ingest including memory
python3 qdrant_ingest.py --all --memory

# Search
python3 qdrant_search.py "what did we discuss about qdrant?"
python3 qdrant_search.py --stats
```

## Why Python (not Node.js)?

The Node.js equivalents in `rcc/vector/index.mjs` exist for the RCC dashboard.
These Python scripts are designed for Hermes agent integration — they read from
`~/.hermes/state.db` directly, work with Hermes cron jobs, and have zero external
dependencies (important for agent environments where pip isn't always available).

Both implementations talk to the same Qdrant collections and are interoperable.
