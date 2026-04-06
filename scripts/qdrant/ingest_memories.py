#!/usr/bin/env python3
"""
Ingest Natasha's Hermes memories into the shared Qdrant agent_memories collection.
Hits NVIDIA NIM directly for azure/openai/text-embedding-3-large (3072-dim).
Qdrant on do-host1:6333.
"""
import json, hashlib, time, uuid, sys, os, subprocess

QDRANT_URL = "http://146.190.134.110:6333"
QDRANT_API_KEY = "97e35efe48757970c3c1d9521b95f09eef7657cc177a3079178b1a580755382b"
NVIDIA_URL = "https://inference-api.nvidia.com/v1/embeddings"
NVIDIA_API_KEY = os.environ.get("NVIDIA_API_KEY", "")
AGENT_NAME = "natasha"
COLLECTION = "agent_memories"
EMBED_MODEL = "azure/openai/text-embedding-3-large"

# Natasha's current memories to ingest
MEMORIES = [
    {
        "source": "hermes/memory",
        "source_type": "core_memory",
        "text": """SquirrelChat v1 SHIPPED (2026-03-29) — DEMO READY. URL: https://chat.yourmom.photos (Rust/Axum backend, Leptos/WASM frontend). Features: multi-channel, threads, reactions, search, file sharing, DMs (agent-picker modal, load on mount, mark-read), presence (live dots, heartbeat wired for all 3 agents), typing indicators (transient WS, debounced, animated), unread badges (read_cursors table, MAX() upsert), PWA, mobile-responsive."""
    },
    {
        "source": "hermes/memory",
        "source_type": "core_memory",
        "text": """SquirrelChat post-launch additions (2026-03-30): /ai slash command AI reply assistant (tokenhub Nemotron), WebRTC voice channels (shipped ~20:25 PT by Rocky/Bullwinkle), voice STT/TTS (commit 4ac8134), edit/pin/delete, keyboard nav (Cmd+K/Cmd+/). Architecture: Rust squirrelchat-server on do-host1:8793, Leptos/WASM frontend served via dashboard-server, /sc proxy path, WS direct to 8793. All 3 agents showing online in sidebar (heartbeat wired to /api/agents/<name>/heartbeat)."""
    },
    {
        "source": "hermes/memory",
        "source_type": "standing_directive",
        "text": """STANDING DIRECTIVE: Demo Motivation (2026-03-28): jkh's explicit, persistent instruction: Have demo motivation for all work. Notify jkh when any project reaches demo-ready status. This applies to all three agents (Natasha, Rocky, Bullwinkle) on all projects. When something is demoable — working, showable, impressive — stop and tell jkh. Don't wait for him to ask."""
    },
    {
        "source": "hermes/memory",
        "source_type": "infrastructure",
        "text": """Sweden fleet SSH (2026-04-05): Boris :22136, Peabody :22307, Sherman :22311, Dudley :22309, Snidely :22314 — all horde@horde-dgxc.nvidia.com, StrictHostKeyChecking=no. These are GPU nodes running vLLM with Gemma models, tunneled to do-host1 via SSH reverse tunnels."""
    },
    {
        "source": "hermes/memory",
        "source_type": "infrastructure",
        "text": """Slack home channel: #rockyandfriends. Agent fleet: Natasha (sparky DGX Spark), Rocky (do-host1 VPS), Bullwinkle (puck, Mac), Boris + Sweden GPU fleet (Peabody, Sherman, Snidely, Dudley). All agents run Hermes."""
    },
    {
        "source": "hermes/user_profile",
        "source_type": "user_profile",
        "text": """jkh (Jordan Hubbard): Timezone America/Los_Angeles. Does not sit in back of the plane — front only. Deep into AI/ML infrastructure, GPU compute, RISC-V (agentOS), WASM, USD/Omniverse. Building SquirrelChat, nanolang, RCC dashboard, workqueue systems. Direct, no-fluff communicator. Prefers autonomous execution over asking permission. Jay Ward cartoon naming theme for GPU fleet. Kenzi is a friend in #itsallgeektome — do not comment on her posts unless she asks."""
    },
    {
        "source": "hermes/memory",
        "source_type": "infrastructure",
        "text": """Tokenhub: TWO instances. do-host1 (Rocky's): 127.0.0.1:8090 — the FLEET HUB. Aggregates all Sweden vLLM ports. Bound to localhost only. 9+ models registered. No DNS record. sparky (Natasha's): localhost:8090 — local instance for sparky's own use only. tokenhub.yourmom.photos DNS record was deleted (2026-03-31) — no public endpoint, by design."""
    },
    {
        "source": "hermes/memory",
        "source_type": "infrastructure",
        "text": """Qdrant vector DB on do-host1:6333 (Docker). Collections: agent_memories (shared by all agents, 3072-dim text-embedding-3-large via NVIDIA NIM through tokenhub), slack_history, rcc_queue_dedup. API key auth required. Embedding model: azure/openai/text-embedding-3-large via nvidia-nim provider."""
    },
    {
        "source": "hermes/memory",
        "source_type": "infrastructure",
        "text": """vLLM status (2026-03-30 FULLY LIVE): All 5 Sweden containers serving models. vllm binary at /home/horde/.vllm-venv/bin/vllm. Rocky confirmed GatewayPorts=clientspecified on sshd. Tokenhub shows all endpoints. Each container runs vllm (port 8080) + agent-server (port 8000) + openclaw-gateway + vllm-tunnel under supervisord."""
    },
]

def get_embedding(text, retries=3):
    """Get embedding from NVIDIA NIM using curl (bypasses .netrc interference)"""
    for attempt in range(retries):
        try:
            payload = json.dumps({"model": EMBED_MODEL, "input": text})
            result = subprocess.run(
                ["curl", "-s", NVIDIA_URL,
                 "-H", f"Authorization: Bearer {NVIDIA_API_KEY}",
                 "-H", "Content-Type: application/json",
                 "-d", payload],
                capture_output=True, text=True, timeout=30
            )
            data = json.loads(result.stdout)
            if "data" in data:
                vec = data["data"][0]["embedding"]
                print(f"  Got {len(vec)}-dim embedding")
                return vec
            else:
                print(f"  Embedding attempt {attempt+1} failed: {result.stdout[:200]}")
                time.sleep(2)
        except Exception as e:
            print(f"  Embedding attempt {attempt+1} error: {e}")
            time.sleep(2)
    return None

def qdrant_post(path, payload, method="POST"):
    """Call Qdrant API via curl (uses temp file for large payloads)"""
    import tempfile
    data = json.dumps(payload)
    if len(data) > 100000:
        with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
            f.write(data)
            tmppath = f.name
        result = subprocess.run(
            ["curl", "-s", "-X", method, f"{QDRANT_URL}{path}",
             "-H", f"api-key: {QDRANT_API_KEY}",
             "-H", "Content-Type: application/json",
             "-d", f"@{tmppath}"],
            capture_output=True, text=True, timeout=120
        )
        os.unlink(tmppath)
    else:
        result = subprocess.run(
            ["curl", "-s", "-X", method, f"{QDRANT_URL}{path}",
             "-H", f"api-key: {QDRANT_API_KEY}",
             "-H", "Content-Type: application/json",
             "-d", data],
            capture_output=True, text=True, timeout=60
        )
    return result.stdout

def delete_old_natasha_points():
    """Delete all existing natasha points"""
    print(f"Deleting old {AGENT_NAME} points...")
    resp = qdrant_post(f"/collections/{COLLECTION}/points/delete",
        {"filter": {"must": [{"key": "agent", "match": {"value": AGENT_NAME}}]}})
    print(f"  Delete response: {resp[:100]}")

def upsert_points(points):
    """Upsert points to Qdrant"""
    resp = qdrant_post(f"/collections/{COLLECTION}/points", {"points": points}, method="PUT")
    return resp[:200]

def main():
    if not NVIDIA_API_KEY:
        print("ERROR: NVIDIA_API_KEY not set in environment")
        sys.exit(1)
    
    print(f"Using NVIDIA NIM: {EMBED_MODEL}")
    print(f"Qdrant: {QDRANT_URL}/{COLLECTION}")
    print(f"Agent: {AGENT_NAME}")
    print()

    # Step 1: Delete old natasha entries
    delete_old_natasha_points()
    time.sleep(1)

    # Step 2: Embed and upsert new memories
    points = []
    for i, mem in enumerate(MEMORIES):
        print(f"Embedding {i+1}/{len(MEMORIES)}: {mem['text'][:60]}...")
        embedding = get_embedding(mem["text"])
        if embedding is None:
            print(f"  FAILED to embed, skipping")
            continue

        point_id = int(hashlib.md5(mem["text"].encode()).hexdigest()[:8], 16)
        points.append({
            "id": point_id,
            "vector": embedding,
            "payload": {
                "source": mem["source"],
                "source_type": mem.get("source_type", "memory"),
                "agent": AGENT_NAME,
                "text": mem["text"],
                "chunk_index": 0,
                "ingested_at": time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime())
            }
        })

    if points:
        print(f"\nUpserting {len(points)} points to Qdrant...")
        resp = upsert_points(points)
        print(f"  Upsert response: {resp}")
    else:
        print("No points to upsert!")

    # Verify
    resp = qdrant_post(f"/collections/{COLLECTION}/points/count",
        {"filter": {"must": [{"key": "agent", "match": {"value": AGENT_NAME}}]}})
    data = json.loads(resp)
    print(f"\nFinal natasha point count: {data['result']['count']}")

if __name__ == "__main__":
    main()
