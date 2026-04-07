#!/usr/bin/env python3
"""
Search Qdrant vector database for Hermes agent data.

Usage:
    python3 qdrant_search.py "what did we discuss about qdrant?"
    python3 qdrant_search.py "docker networking" --collection hermes_sessions --limit 10
    python3 qdrant_search.py "user preferences" --collection agent_memories
    python3 qdrant_search.py "slack conversation about deployment" --collection slack_history
    python3 qdrant_search.py --stats   # Show collection stats

Searches across hermes_sessions (default), agent_memories, and slack_history.
"""

import sys
import os
import json
import argparse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qdrant_common import (
    get_qdrant_api_key, get_tokenhub_api_key,
    search, qdrant_get,
    COLLECTION_SESSIONS, COLLECTION_MEMORIES,
)

COLLECTIONS = [COLLECTION_SESSIONS, COLLECTION_MEMORIES, "slack_history"]


def show_stats(api_key):
    """Show stats for all collections."""
    print("=== Qdrant Collection Stats ===\n")
    for name in COLLECTIONS:
        try:
            info = qdrant_get(f"/collections/{name}", api_key=api_key)
            r = info["result"]
            pts = r["points_count"]
            status = r["status"]
            dims = r["config"]["params"]["vectors"]["size"]
            print(f"  {name}: {pts} points, {dims}-dim, status={status}")
        except Exception as e:
            print(f"  {name}: ERROR - {e}")
    print()


def search_and_display(query, collection, limit, api_key, tokenhub_key, filters=None):
    """Search a collection and display results."""
    results = search(
        collection, query, limit=limit,
        filters=filters, api_key=api_key, tokenhub_key=tokenhub_key,
    )
    
    if not results:
        print(f"  No results in {collection}")
        return []
    
    for i, r in enumerate(results):
        score = r["score"]
        p = r["payload"]
        
        print(f"\n--- Result {i+1} (score: {score:.4f}) [{collection}] ---")
        
        # Display based on chunk type / collection
        if collection == "slack_history":
            channel = p.get("channel_name", "?")
            user = p.get("user", "?")
            ts = p.get("ts", "?")
            text = p.get("text", "")
            print(f"  Channel: #{channel}  User: {user}  TS: {ts}")
            print(f"  {text[:500]}")
        elif collection == COLLECTION_SESSIONS:
            sid = p.get("session_id", "?")
            source = p.get("source", "?")
            started = p.get("started_at", "?")
            chunk_idx = p.get("chunk_index", "?")
            total = p.get("total_chunks", "?")
            text = p.get("text", "")
            print(f"  Session: {sid}  Source: {source}  Started: {started}")
            print(f"  Chunk: {chunk_idx}/{total}")
            # Show the text, truncated
            lines = text.split("\n")
            # Skip header lines (Session:, Agent:, etc.)
            content_start = 0
            for j, line in enumerate(lines):
                if line.strip() == "" and j > 0:
                    content_start = j + 1
                    break
            content = "\n".join(lines[content_start:])[:600]
            print(f"  {content}")
        else:
            # agent_memories or generic
            agent = p.get("agent", "?")
            chunk_type = p.get("chunk_type", p.get("source_type", "?"))
            text = p.get("text", "")
            print(f"  Agent: {agent}  Type: {chunk_type}")
            print(f"  {text[:500]}")
    
    return results


def main():
    parser = argparse.ArgumentParser(description="Search Qdrant for Hermes data")
    parser.add_argument("query", nargs="?", help="Search query text")
    parser.add_argument("--collection", "-c", type=str, default=None,
                       help=f"Collection to search (default: search all). Options: {', '.join(COLLECTIONS)}")
    parser.add_argument("--limit", "-n", type=int, default=5, help="Max results per collection")
    parser.add_argument("--stats", action="store_true", help="Show collection stats")
    parser.add_argument("--agent", type=str, help="Filter by agent name")
    parser.add_argument("--source", type=str, help="Filter by source (cli, slack, etc)")
    parser.add_argument("--json", action="store_true", help="Output as JSON")
    args = parser.parse_args()
    
    api_key = get_qdrant_api_key()
    tokenhub_key = get_tokenhub_api_key()
    
    if args.stats:
        show_stats(api_key)
        return
    
    if not args.query:
        parser.print_help()
        sys.exit(1)
    
    # Build filters
    filters = None
    must_conditions = []
    if args.agent:
        must_conditions.append({"key": "agent", "match": {"value": args.agent}})
    if args.source:
        must_conditions.append({"key": "source", "match": {"value": args.source}})
    if must_conditions:
        filters = {"must": must_conditions}
    
    # Determine which collections to search
    if args.collection:
        collections_to_search = [args.collection]
    else:
        # Search all that exist
        collections_to_search = []
        for name in COLLECTIONS:
            try:
                info = qdrant_get(f"/collections/{name}", api_key=api_key)
                if info["result"]["points_count"] > 0:
                    collections_to_search.append(name)
            except Exception:
                pass
    
    if not args.json:
        print(f'=== Searching: "{args.query}" ===')
        print(f"  Collections: {', '.join(collections_to_search)}")
    
    all_results = {}
    for coll in collections_to_search:
        try:
            results = search_and_display(
                args.query, coll, args.limit,
                api_key, tokenhub_key, filters=filters
            )
            all_results[coll] = results
        except Exception as e:
            if not args.json:
                print(f"\n  Error searching {coll}: {e}")
    
    if args.json:
        print(json.dumps(all_results, indent=2))
    elif not any(all_results.values()):
        print("\n  No results found in any collection.")


if __name__ == "__main__":
    main()
