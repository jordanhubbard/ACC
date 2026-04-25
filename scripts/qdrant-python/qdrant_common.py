"""
Shared Qdrant + embedding utilities for Hermes agent integration.
Uses only stdlib (urllib, json, hashlib) — no pip dependencies.
"""

import json
import os
import hashlib
import subprocess
import urllib.request
import urllib.error
import time
import sys


# ── Configuration ──────────────────────────────────────────────────

QDRANT_URL = os.environ.get("QDRANT_URL", "http://localhost:6333")

# Embeddings go through tokenhub — never call NVIDIA directly.
# This ensures we benefit from tokenhub's provider routing, rate limiting,
# and future backends (e.g. Sweden fleet embedding models).
TOKENHUB_URL = os.environ.get("TOKENHUB_URL", "http://localhost:8090")
EMBEDDING_URL = f"{TOKENHUB_URL}/v1/embeddings"
EMBEDDING_MODEL = "text-embedding-3-large"
EMBEDDING_DIM = 3072
EMBED_BATCH_SIZE = 20

COLLECTION_SESSIONS = "hermes_sessions"
COLLECTION_MEMORIES = "agent_memories"


def get_qdrant_api_key():
    """Get Qdrant API key from Docker container env."""
    result = subprocess.run(
        ["docker", "inspect", "qdrant"],
        capture_output=True, text=True, timeout=10
    )
    config = json.loads(result.stdout)
    for env in config[0]["Config"]["Env"]:
        if "API_KEY" in env:
            return env.split("=", 1)[1]
    raise RuntimeError("Could not find Qdrant API key in container env")


def get_tokenhub_api_key():
    """Get tokenhub API key from env or Hermes .env file."""
    # Check env var first
    key = os.environ.get("TOKENHUB_API_KEY", "")
    if key:
        return key

    # Fall back to Hermes .env (look for CUSTOM_*_8090_API_KEY or TOKENHUB_API_KEY)
    env_path = os.path.expanduser("~/.hermes/.env")
    try:
        with open(env_path) as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                for prefix in ["TOKENHUB_API_KEY", "CUSTOM_146_190_134_110_8090_API_KEY"]:
                    if line.startswith(prefix + "="):
                        val = line.split("=", 1)[1]
                        if val.startswith('"') and val.endswith('"'):
                            val = val[1:-1]
                        if val.startswith("'") and val.endswith("'"):
                            val = val[1:-1]
                        return val
    except FileNotFoundError:
        pass
    raise RuntimeError("No tokenhub API key found in env or ~/.hermes/.env")


# ── HTTP Helpers ───────────────────────────────────────────────────

def qdrant_request(method, path, data=None, api_key=None):
    """Make an HTTP request to Qdrant."""
    if api_key is None:
        api_key = get_qdrant_api_key()
    
    url = f"{QDRANT_URL}{path}"
    headers = {
        "api-key": api_key,
        "Content-Type": "application/json",
    }
    
    body = json.dumps(data).encode() if data else None
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        error_body = e.read().decode() if e.fp else ""
        raise RuntimeError(f"Qdrant {method} {path} failed ({e.code}): {error_body}")


def qdrant_get(path, api_key=None):
    return qdrant_request("GET", path, api_key=api_key)


def qdrant_post(path, data, api_key=None):
    return qdrant_request("POST", path, data=data, api_key=api_key)


def qdrant_put(path, data, api_key=None):
    return qdrant_request("PUT", path, data=data, api_key=api_key)


# ── Embedding ──────────────────────────────────────────────────────

def get_embeddings(texts, tokenhub_key=None, retries=3):
    """Get embeddings for a list of texts via tokenhub.
    
    Returns list of embedding vectors (each a list of floats).
    Handles batching automatically.
    """
    if tokenhub_key is None:
        tokenhub_key = get_tokenhub_api_key()
    
    all_embeddings = []
    
    for i in range(0, len(texts), EMBED_BATCH_SIZE):
        batch = texts[i:i + EMBED_BATCH_SIZE]
        
        for attempt in range(retries):
            try:
                req_data = json.dumps({
                    "model": EMBEDDING_MODEL,
                    "input": batch
                }).encode()
                
                req = urllib.request.Request(
                    EMBEDDING_URL,
                    data=req_data,
                    headers={
                        "Content-Type": "application/json",
                        "Authorization": f"Bearer {tokenhub_key}",
                    }
                )
                
                with urllib.request.urlopen(req, timeout=60) as resp:
                    result = json.loads(resp.read())
                
                # Sort by index to ensure order
                sorted_data = sorted(result["data"], key=lambda x: x["index"])
                batch_embeddings = [d["embedding"] for d in sorted_data]
                all_embeddings.extend(batch_embeddings)
                break
                
            except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError) as e:
                if attempt < retries - 1:
                    wait = 2 ** attempt
                    print(f"  Embedding request failed ({e}), retrying in {wait}s...")
                    time.sleep(wait)
                else:
                    raise RuntimeError(f"Embedding failed after {retries} attempts: {e}")
    
    return all_embeddings


def get_single_embedding(text, tokenhub_key=None):
    """Get embedding for a single text string."""
    return get_embeddings([text], tokenhub_key=tokenhub_key)[0]


# ── Collection Management ─────────────────────────────────────────

def ensure_collection(name, dim=EMBEDDING_DIM, api_key=None):
    """Create collection if it doesn't exist."""
    if api_key is None:
        api_key = get_qdrant_api_key()
    
    # Check if exists
    try:
        info = qdrant_get(f"/collections/{name}", api_key=api_key)
        return info["result"]["points_count"]
    except RuntimeError:
        pass
    
    # Create it
    qdrant_put(f"/collections/{name}", {
        "vectors": {
            "size": dim,
            "distance": "Cosine"
        },
        "optimizers_config": {
            "indexing_threshold": 1000
        }
    }, api_key=api_key)
    
    # Create payload indexes for common query patterns
    for field in ["session_id", "agent", "source", "role", "chunk_type"]:
        try:
            qdrant_put(f"/collections/{name}/index", {
                "field_name": field,
                "field_schema": "keyword"
            }, api_key=api_key)
        except RuntimeError:
            pass  # Index may already exist
    
    print(f"  Created collection '{name}' ({dim}-dim, Cosine)")
    return 0


def collection_info(name, api_key=None):
    """Get collection point count and status."""
    info = qdrant_get(f"/collections/{name}", api_key=api_key)
    return {
        "points": info["result"]["points_count"],
        "status": info["result"]["status"],
    }


# ── Chunking ──────────────────────────────────────────────────────

def chunk_text(text, max_chars=1500, overlap=200):
    """Split text into overlapping chunks by paragraph boundaries."""
    if not text or not text.strip():
        return []
    
    paragraphs = text.split("\n\n")
    chunks = []
    current = ""
    
    for para in paragraphs:
        para = para.strip()
        if not para:
            continue
        
        if len(current) + len(para) + 2 > max_chars and current:
            chunks.append(current.strip())
            # Keep overlap from end of current chunk
            if overlap > 0 and len(current) > overlap:
                current = current[-overlap:] + "\n\n" + para
            else:
                current = para
        else:
            current = (current + "\n\n" + para).strip() if current else para
    
    if current.strip():
        chunks.append(current.strip())
    
    return chunks if chunks else [text.strip()[:max_chars]]


# ── Point ID Generation ──────────────────────────────────────────

def deterministic_point_id(namespace, *parts):
    """Generate a deterministic UUID-like integer ID from parts.
    
    Qdrant accepts both UUIDs and unsigned 64-bit ints as point IDs.
    We use a hash to create deterministic IDs so re-ingestion is idempotent.
    """
    key = ":".join([namespace] + [str(p) for p in parts])
    h = hashlib.md5(key.encode()).hexdigest()
    # Use first 16 hex chars = 64 bits, but keep it positive (63 bits)
    return int(h[:16], 16) & 0x7FFFFFFFFFFFFFFF


# ── Upsert Helper ─────────────────────────────────────────────────

def upsert_points(collection, points, api_key=None, batch_size=100):
    """Upsert points to Qdrant in batches.
    
    points: list of {"id": int, "vector": [...], "payload": {...}}
    """
    if api_key is None:
        api_key = get_qdrant_api_key()
    
    total = len(points)
    upserted = 0
    
    for i in range(0, total, batch_size):
        batch = points[i:i + batch_size]
        qdrant_put(f"/collections/{collection}/points", {
            "points": batch
        }, api_key=api_key)
        upserted += len(batch)
        if total > batch_size:
            print(f"  Upserted {upserted}/{total} points...")
    
    return upserted


# ── Search ────────────────────────────────────────────────────────

def search(collection, query_text, limit=5, filters=None, api_key=None, tokenhub_key=None):
    """Semantic search over a collection.
    
    Returns list of {"score": float, "payload": dict}
    """
    if api_key is None:
        api_key = get_qdrant_api_key()
    
    query_vector = get_single_embedding(query_text, tokenhub_key=tokenhub_key)
    
    search_body = {
        "vector": query_vector,
        "limit": limit,
        "with_payload": True,
    }
    
    if filters:
        search_body["filter"] = filters
    
    result = qdrant_post(f"/collections/{collection}/points/search", search_body, api_key=api_key)
    
    return [
        {"score": pt["score"], "payload": pt["payload"]}
        for pt in result["result"]
    ]
