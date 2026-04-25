#!/usr/bin/env python3
"""
Ingest Hermes session data into Qdrant vector database.

Usage:
    python3 qdrant_ingest.py                    # Ingest new sessions only
    python3 qdrant_ingest.py --all              # Re-ingest everything
    python3 qdrant_ingest.py --memory           # Also ingest MEMORY.md + user profile
    python3 qdrant_ingest.py --session ID       # Ingest specific session
    python3 qdrant_ingest.py --all --memory     # Full re-ingest including memory

Reads from ~/.hermes/state.db (sessions + messages).
Writes to Qdrant collection 'hermes_sessions'.
Uses NVIDIA NIM text-embedding-3-large (3072-dim).

Idempotent: uses deterministic point IDs so re-runs update existing points.
"""

import sys
import os
import json
import sqlite3
import time
import argparse
from datetime import datetime

# Add scripts dir to path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qdrant_common import (
    get_qdrant_api_key, get_tokenhub_api_key,
    ensure_collection, get_embeddings, upsert_points,
    chunk_text, deterministic_point_id,
    qdrant_post, qdrant_get,
    COLLECTION_SESSIONS, COLLECTION_MEMORIES,
)

AGENT_NAME = os.environ.get("AGENT_NAME", "Rocky")
STATE_DB = os.path.expanduser("~/.hermes/state.db")
MEMORY_FILE = os.path.expanduser("~/.hermes/MEMORY.md")
USER_FILE = os.path.expanduser("~/.hermes/USER.md")
SOUL_FILE = os.path.expanduser("~/.hermes/SOUL.md")

# Track what's already ingested
INGEST_STATE_FILE = os.path.expanduser("~/.hermes/scripts/.qdrant_ingest_state.json")


def load_ingest_state():
    """Load set of already-ingested session IDs."""
    if os.path.exists(INGEST_STATE_FILE):
        with open(INGEST_STATE_FILE) as f:
            return json.load(f)
    return {"ingested_sessions": [], "last_run": None}


def save_ingest_state(state):
    """Save ingest state."""
    state["last_run"] = datetime.utcnow().isoformat() + "Z"
    with open(INGEST_STATE_FILE, "w") as f:
        json.dump(state, f, indent=2)


def get_sessions(db_path, session_id=None):
    """Get session metadata from state.db."""
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    
    if session_id:
        cursor.execute("SELECT * FROM sessions WHERE id = ?", (session_id,))
    else:
        cursor.execute("SELECT * FROM sessions ORDER BY started_at")
    
    sessions = [dict(row) for row in cursor.fetchall()]
    conn.close()
    return sessions


def get_messages(db_path, session_id):
    """Get messages for a session from state.db."""
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    
    cursor.execute(
        "SELECT * FROM messages WHERE session_id = ? ORDER BY timestamp",
        (session_id,)
    )
    messages = [dict(row) for row in cursor.fetchall()]
    conn.close()
    return messages


def build_session_document(session, messages):
    """Build a single text document from a session's messages.
    
    Returns a list of chunks, each with metadata.
    """
    session_id = session["id"]
    source = session.get("source", "unknown")
    model = session.get("model", "unknown")
    started = session.get("started_at", 0)
    title = session.get("title", "")
    
    if started:
        started_str = datetime.fromtimestamp(started).strftime("%Y-%m-%d %H:%M:%S")
    else:
        started_str = "unknown"
    
    # Build conversation text
    conversation_parts = []
    for msg in messages:
        role = msg["role"]
        content = msg.get("content", "")
        tool_name = msg.get("tool_name", "")
        
        if not content or not content.strip():
            continue
        
        # Skip raw tool call JSON and large tool outputs
        if role == "tool" and len(content) > 2000:
            # Summarize long tool outputs
            content = content[:500] + f"\n... [{len(content)} chars total]"
        
        if role == "assistant" and msg.get("tool_calls"):
            # Skip assistant messages that are just tool call invocations
            if not content.strip():
                continue
        
        prefix = {
            "user": "User",
            "assistant": f"Assistant ({AGENT_NAME})",
            "system": "System",
            "tool": f"Tool ({tool_name})" if tool_name else "Tool",
        }.get(role, role)
        
        conversation_parts.append(f"{prefix}: {content}")
    
    if not conversation_parts:
        return []
    
    full_text = "\n\n".join(conversation_parts)
    
    # Create a session header for context
    header = f"Session: {session_id}\nAgent: {AGENT_NAME}\nSource: {source}\nModel: {model}\nStarted: {started_str}"
    if title:
        header += f"\nTitle: {title}"
    
    # Chunk the conversation
    chunks = chunk_text(full_text, max_chars=1500, overlap=200)
    
    result = []
    for i, chunk in enumerate(chunks):
        point_id = deterministic_point_id("hermes_session", session_id, i)
        text_with_context = f"{header}\n\n{chunk}"
        
        result.append({
            "id": point_id,
            "text": text_with_context,
            "payload": {
                "session_id": session_id,
                "agent": AGENT_NAME,
                "source": source,
                "model": model,
                "started_at": started_str,
                "title": title or "",
                "chunk_index": i,
                "total_chunks": len(chunks),
                "chunk_type": "session",
                "text": text_with_context,
                "ingested_at": datetime.utcnow().isoformat() + "Z",
            }
        })
    
    return result


def build_memory_chunks(memory_text, memory_type="memory"):
    """Chunk a memory document (MEMORY.md or USER.md) for ingestion."""
    if not memory_text or not memory_text.strip():
        return []
    
    chunks = chunk_text(memory_text, max_chars=1000, overlap=100)
    result = []
    
    for i, chunk in enumerate(chunks):
        point_id = deterministic_point_id(f"hermes_{memory_type}", AGENT_NAME, i)
        header = f"Agent: {AGENT_NAME}\nType: {memory_type}\n\n"
        text = header + chunk
        
        result.append({
            "id": point_id,
            "text": text,
            "payload": {
                "agent": AGENT_NAME,
                "chunk_type": memory_type,
                "chunk_index": i,
                "total_chunks": len(chunks),
                "text": text,
                "source": f"{memory_type}.md",
                "ingested_at": datetime.utcnow().isoformat() + "Z",
            }
        })
    
    return result


def ingest_sessions(sessions, messages_by_session, api_key, tokenhub_key):
    """Ingest session data into Qdrant."""
    all_chunks = []
    
    for session in sessions:
        sid = session["id"]
        messages = messages_by_session.get(sid, [])
        if not messages:
            continue
        
        chunks = build_session_document(session, messages)
        all_chunks.extend(chunks)
    
    if not all_chunks:
        print("  No chunks to ingest.")
        return 0
    
    print(f"  {len(all_chunks)} chunks from {len(sessions)} sessions")
    
    # Get embeddings in batches
    texts = [c["text"] for c in all_chunks]
    print(f"  Generating embeddings for {len(texts)} chunks...")
    embeddings = get_embeddings(texts, tokenhub_key=tokenhub_key)
    
    # Build points
    points = []
    for chunk, embedding in zip(all_chunks, embeddings):
        points.append({
            "id": chunk["id"],
            "vector": embedding,
            "payload": chunk["payload"],
        })
    
    # Upsert to Qdrant
    print(f"  Upserting {len(points)} points to {COLLECTION_SESSIONS}...")
    upserted = upsert_points(COLLECTION_SESSIONS, points, api_key=api_key)
    return upserted


def ingest_memory_files(api_key, tokenhub_key):
    """Ingest MEMORY.md and USER.md into agent_memories collection."""
    all_chunks = []
    
    for filepath, mtype in [(MEMORY_FILE, "memory"), (USER_FILE, "user_profile"), (SOUL_FILE, "soul")]:
        if os.path.exists(filepath):
            with open(filepath) as f:
                text = f.read()
            chunks = build_memory_chunks(text, memory_type=mtype)
            all_chunks.extend(chunks)
            print(f"  {filepath}: {len(chunks)} chunks")
        else:
            print(f"  {filepath}: not found, skipping")
    
    if not all_chunks:
        return 0
    
    texts = [c["text"] for c in all_chunks]
    print(f"  Generating embeddings for {len(texts)} memory chunks...")
    embeddings = get_embeddings(texts, tokenhub_key=tokenhub_key)
    
    points = []
    for chunk, embedding in zip(all_chunks, embeddings):
        points.append({
            "id": chunk["id"],
            "vector": embedding,
            "payload": chunk["payload"],
        })
    
    print(f"  Upserting {len(points)} points to {COLLECTION_MEMORIES}...")
    upserted = upsert_points(COLLECTION_MEMORIES, points, api_key=api_key)
    return upserted


def main():
    parser = argparse.ArgumentParser(description="Ingest Hermes data into Qdrant")
    parser.add_argument("--all", action="store_true", help="Re-ingest all sessions")
    parser.add_argument("--memory", action="store_true", help="Also ingest MEMORY.md/USER.md")
    parser.add_argument("--session", type=str, help="Ingest specific session ID")
    args = parser.parse_args()
    
    print(f"=== Qdrant Ingestion for {AGENT_NAME} ===")
    print(f"  DB: {STATE_DB}")
    
    # Get credentials
    api_key = get_qdrant_api_key()
    tokenhub_key = get_tokenhub_api_key()
    print("  Credentials loaded ✓")
    
    # Ensure collections exist
    sess_count = ensure_collection(COLLECTION_SESSIONS, api_key=api_key)
    mem_count = ensure_collection(COLLECTION_MEMORIES, api_key=api_key)
    print(f"  {COLLECTION_SESSIONS}: {sess_count} existing points")
    print(f"  {COLLECTION_MEMORIES}: {mem_count} existing points")
    
    # Load ingest state
    state = load_ingest_state()
    
    # Get sessions to ingest
    if args.session:
        sessions = get_sessions(STATE_DB, session_id=args.session)
        print(f"\n  Ingesting specific session: {args.session}")
    elif args.all:
        sessions = get_sessions(STATE_DB)
        print(f"\n  Re-ingesting ALL {len(sessions)} sessions")
    else:
        # Only new sessions
        already = set(state.get("ingested_sessions", []))
        all_sessions = get_sessions(STATE_DB)
        sessions = [s for s in all_sessions if s["id"] not in already]
        print(f"\n  {len(sessions)} new sessions to ingest ({len(already)} already done)")
    
    # Load messages for sessions
    conn = sqlite3.connect(STATE_DB)
    conn.row_factory = sqlite3.Row
    cursor = conn.cursor()
    
    messages_by_session = {}
    for session in sessions:
        sid = session["id"]
        cursor.execute(
            "SELECT * FROM messages WHERE session_id = ? ORDER BY timestamp",
            (sid,)
        )
        messages_by_session[sid] = [dict(row) for row in cursor.fetchall()]
    conn.close()
    
    # Ingest sessions
    if sessions:
        t0 = time.time()
        count = ingest_sessions(sessions, messages_by_session, api_key, tokenhub_key)
        elapsed = time.time() - t0
        print(f"  ✓ Ingested {count} session points in {elapsed:.1f}s")
        
        # Update state
        for s in sessions:
            if s["id"] not in state.get("ingested_sessions", []):
                state.setdefault("ingested_sessions", []).append(s["id"])
        save_ingest_state(state)
    else:
        print("  No sessions to ingest.")
    
    # Ingest memory files if requested
    if args.memory:
        print("\n  Ingesting memory files...")
        t0 = time.time()
        mem_count = ingest_memory_files(api_key, tokenhub_key)
        elapsed = time.time() - t0
        print(f"  ✓ Ingested {mem_count} memory points in {elapsed:.1f}s")
    
    # Final stats
    sess_info = qdrant_get(f"/collections/{COLLECTION_SESSIONS}", api_key=api_key)
    mem_info = qdrant_get(f"/collections/{COLLECTION_MEMORIES}", api_key=api_key)
    print(f"\n=== Final State ===")
    print(f"  {COLLECTION_SESSIONS}: {sess_info['result']['points_count']} points")
    print(f"  {COLLECTION_MEMORIES}: {mem_info['result']['points_count']} points")
    print("  Done!")


if __name__ == "__main__":
    main()
