"""
DAVA Knowledge Index — fast pre-computed keyword lookup
Builds an inverted index from knowledge.jsonl so searches are O(1) per keyword
instead of scanning all 1000+ chunks every query.

Usage:
    python knowledge_index.py --build    # Build/rebuild the index
    python knowledge_index.py --search "consciousness qualia"
    python knowledge_index.py --stats

Called automatically by memory_store.py on first search if index doesn't exist.
"""
import json
import os
import sys
import time
from collections import defaultdict
from pathlib import Path

MEMORY_DIR = Path(__file__).parent / "dava_memory"
KNOWLEDGE_FILE = MEMORY_DIR / "knowledge.jsonl"
INDEX_FILE = MEMORY_DIR / "knowledge_index.json"

# Common words to skip
STOPWORDS = frozenset(
    "the a an is are was were be been being have has had do does did will would "
    "shall should may might can could of in to for on with at by from as into "
    "through during before after above below between out off over under again "
    "further then once here there when where why how all each every both few "
    "more most other some such no nor not only own same so than too very and "
    "but or if while that this these those it its he she they them their his "
    "her which what who whom whose also just about up down any many much like".split()
)


def tokenize(text):
    """Split text into lowercase keywords, filter stopwords and short words."""
    words = []
    for w in text.lower().split():
        # Strip punctuation
        w = w.strip(".,;:!?\"'()-[]{}/*#@&")
        if len(w) > 2 and w not in STOPWORDS:
            words.append(w)
    return words


def build_index():
    """Build inverted index from knowledge.jsonl."""
    if not KNOWLEDGE_FILE.exists():
        print("No knowledge.jsonl found")
        return {}

    # inverted_index: keyword -> list of (chunk_id, score)
    # where score = 1 for body match, 2 for topic match
    inv = defaultdict(list)
    chunks = []
    topic_map = {}  # chunk_id -> topic

    with open(KNOWLEDGE_FILE, "r", encoding="utf-8") as f:
        for i, line in enumerate(f):
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue

            chunk_text = entry.get("chunk", "")
            topic = entry.get("topic", "")
            source = entry.get("source", "")

            chunks.append({
                "id": i,
                "topic": topic,
                "source": source,
                "preview": chunk_text[:120],
                "length": len(chunk_text),
            })
            topic_map[i] = topic

            # Index topic words (weight 3)
            for word in tokenize(topic):
                inv[word].append((i, 3))

            # Index source words (weight 2)
            for word in tokenize(source):
                inv[word].append((i, 2))

            # Index body words (weight 1) — only unique words per chunk
            seen = set()
            for word in tokenize(chunk_text):
                if word not in seen:
                    inv[word].append((i, 1))
                    seen.add(word)

    # Deduplicate and sort entries per keyword
    for word in inv:
        # Keep unique chunk_ids, sum weights
        chunk_scores = defaultdict(int)
        for chunk_id, weight in inv[word]:
            chunk_scores[chunk_id] += weight
        inv[word] = sorted(chunk_scores.items(), key=lambda x: -x[1])

    index = {
        "version": 2,
        "built_at": time.time(),
        "total_chunks": len(chunks),
        "total_keywords": len(inv),
        "chunks": chunks,
        "inverted": {k: v for k, v in inv.items()},
    }

    MEMORY_DIR.mkdir(parents=True, exist_ok=True)
    with open(INDEX_FILE, "w", encoding="utf-8") as f:
        json.dump(index, f)

    print(f"Index built: {len(chunks)} chunks, {len(inv)} keywords")
    return index


# Cache the loaded index in memory
_INDEX_CACHE = {"data": None, "mtime": 0}


def load_index():
    """Load index from disk, rebuild if missing or stale."""
    if not INDEX_FILE.exists():
        build_index()

    if not INDEX_FILE.exists():
        return None

    mtime = os.path.getmtime(INDEX_FILE)
    if _INDEX_CACHE["data"] and _INDEX_CACHE["mtime"] == mtime:
        return _INDEX_CACHE["data"]

    with open(INDEX_FILE, "r", encoding="utf-8") as f:
        data = json.load(f)

    _INDEX_CACHE["data"] = data
    _INDEX_CACHE["mtime"] = mtime
    return data


def search(query, top_k=5):
    """Search the index. Returns list of {topic, source, preview, score}."""
    index = load_index()
    if not index:
        return []

    query_words = tokenize(query)
    if not query_words:
        return []

    # Score each chunk by how many query words hit it
    chunk_scores = defaultdict(int)
    for word in query_words:
        for chunk_id, weight in index.get("inverted", {}).get(word, []):
            chunk_scores[chunk_id] += weight

    # Sort by score descending
    ranked = sorted(chunk_scores.items(), key=lambda x: -x[1])[:top_k]

    results = []
    chunks = index.get("chunks", [])
    for chunk_id, score in ranked:
        if chunk_id < len(chunks):
            c = chunks[chunk_id]
            results.append({
                "topic": c["topic"],
                "source": c["source"],
                "preview": c["preview"],
                "score": score,
                "chunk_id": chunk_id,
            })

    return results


def get_full_chunk(chunk_id):
    """Retrieve the full text of a chunk by ID."""
    if not KNOWLEDGE_FILE.exists():
        return ""
    with open(KNOWLEDGE_FILE, "r", encoding="utf-8") as f:
        for i, line in enumerate(f):
            if i == chunk_id:
                try:
                    return json.loads(line.strip()).get("chunk", "")
                except json.JSONDecodeError:
                    return ""
    return ""


def search_with_text(query, top_k=5):
    """Search and return full chunk text (for context injection)."""
    results = search(query, top_k)
    for r in results:
        r["text"] = get_full_chunk(r["chunk_id"])
    return results


def stats():
    """Print index statistics."""
    index = load_index()
    if not index:
        print("No index found. Run --build first.")
        return

    print(f"=== DAVA Knowledge Index ===")
    print(f"  Chunks:    {index['total_chunks']}")
    print(f"  Keywords:  {index['total_keywords']}")
    print(f"  Built:     {time.ctime(index['built_at'])}")

    # Top 20 most-connected keywords
    inv = index.get("inverted", {})
    top_words = sorted(inv.items(), key=lambda x: -len(x[1]))[:20]
    print(f"\n  Top 20 keywords by chunk coverage:")
    for word, entries in top_words:
        print(f"    {word}: {len(entries)} chunks")


if __name__ == "__main__":
    if "--build" in sys.argv:
        build_index()
    elif "--search" in sys.argv:
        idx = sys.argv.index("--search")
        query = " ".join(sys.argv[idx + 1:])
        results = search_with_text(query, top_k=5)
        for r in results:
            print(f"  [{r['score']}] {r['topic']} ({r['source']})")
            print(f"      {r['text'][:200]}...")
            print()
    elif "--stats" in sys.argv:
        stats()
    else:
        print("Usage: python knowledge_index.py --build | --search <query> | --stats")
