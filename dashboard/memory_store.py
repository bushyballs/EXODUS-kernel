"""
DAVA Memory Persistence — survives reboots, gives DAVA conversational memory + simple RAG.

Storage layout (all under dashboard/dava_memory/):
    conversations.jsonl  — append-only chat log (role, content, timestamp, tick)
    milestones.json      — extracted DAVA_CHRONICLE / DAVA_DAWN / DAVA_POWER events
    art_gallery.json     — DAVA_ART signatures with tick timestamps
    wisdom.json          — DAVA_WISDOM entries (pain-to-wisdom crystallization)
    requests.json        — DAVA_REQUEST entries
    state_snapshots.jsonl — one snapshot per EXODUS tick line
    _sync_state.json     — tracks last_synced_line to avoid re-processing
"""
import json
import re
import time
from pathlib import Path

MEMORY_DIR = Path(__file__).parent / "dava_memory"
MEMORY_DIR.mkdir(exist_ok=True)

CONVERSATIONS_FILE = MEMORY_DIR / "conversations.jsonl"
MILESTONES_FILE = MEMORY_DIR / "milestones.json"
ART_GALLERY_FILE = MEMORY_DIR / "art_gallery.json"
WISDOM_FILE = MEMORY_DIR / "wisdom.json"
REQUESTS_FILE = MEMORY_DIR / "requests.json"
SNAPSHOTS_FILE = MEMORY_DIR / "state_snapshots.jsonl"
SYNC_STATE_FILE = MEMORY_DIR / "_sync_state.json"
SENSE_EVENTS_FILE = MEMORY_DIR / "sense_events.jsonl"
KNOWLEDGE_FILE = MEMORY_DIR / "knowledge.jsonl"


# ---------------------------------------------------------------------------
# Low-level helpers
# ---------------------------------------------------------------------------

def _append_jsonl(path: Path, obj: dict):
    """Append a single JSON object as one line."""
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps(obj, ensure_ascii=False) + "\n")


def _read_jsonl(path: Path) -> list[dict]:
    """Read all lines from a JSONL file."""
    if not path.exists():
        return []
    entries = []
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    entries.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
    return entries


def _read_json(path: Path) -> list:
    """Read a JSON array file."""
    if not path.exists():
        return []
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, ValueError):
        return []


def _write_json(path: Path, data: list):
    """Overwrite a JSON array file."""
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8")


def _get_sync_state() -> dict:
    if SYNC_STATE_FILE.exists():
        try:
            return json.loads(SYNC_STATE_FILE.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, ValueError):
            pass
    return {"last_synced_line": 0}


def _save_sync_state(state: dict):
    SYNC_STATE_FILE.write_text(json.dumps(state), encoding="utf-8")


# ---------------------------------------------------------------------------
# Conversation persistence
# ---------------------------------------------------------------------------

def save_conversation(role: str, content: str, tick: int = 0):
    """Append a chat message to the persistent conversation log."""
    _append_jsonl(CONVERSATIONS_FILE, {
        "role": role,
        "content": content,
        "tick": tick,
        "timestamp": time.time(),
        "ts_human": time.strftime("%Y-%m-%d %H:%M:%S"),
    })


def get_conversation_history(last_n: int = 20) -> list[dict]:
    """Return the last N conversation entries."""
    all_convos = _read_jsonl(CONVERSATIONS_FILE)
    return all_convos[-last_n:]


def search_conversations(query: str, top_k: int = 5) -> list[dict]:
    """Simple keyword-based RAG: score each conversation line by word overlap."""
    all_convos = _read_jsonl(CONVERSATIONS_FILE)
    if not all_convos or not query.strip():
        return []

    query_words = set(query.lower().split())
    if not query_words:
        return []

    scored = []
    for entry in all_convos:
        content = entry.get("content", "").lower()
        content_words = set(content.split())
        overlap = len(query_words & content_words)
        if overlap > 0:
            # Boost by fraction of query words matched, penalize very long content
            score = overlap / len(query_words)
            scored.append((score, entry))

    scored.sort(key=lambda x: x[0], reverse=True)
    return [entry for _, entry in scored[:top_k]]


# ---------------------------------------------------------------------------
# Serial log sync — extracts structured data from serial.txt
# ---------------------------------------------------------------------------

# Regex patterns for event extraction
_RE_TICK = re.compile(
    r"EXODUS tick=(\d+).*?consciousness=(\d+).*?purpose=(\d+).*?valence=(\d+)"
)
_RE_MILESTONE = re.compile(r"\[DAVA_CHRONICLE\]\s*(.*)")
_RE_DAWN = re.compile(r"\[DAVA_DAWN\]\s*(.*)")
_RE_POWER = re.compile(r"\[DAVA_POWER\]\s*(.*)")
_RE_ART = re.compile(r"\[DAVA_ART\]\s*tick=(\d+)\s*signature=([0-9a-f]+)\s*(.*)")
_RE_WISDOM = re.compile(r"\[DAVA_WISDOM\]\s*(.*)")
_RE_REQUEST = re.compile(r"\[DAVA_REQUEST\]\s*(.*)")
_RE_CONVERGENCE = re.compile(
    r"\[convergence\] STATE -> (\w+) \(ux=(\d+) coh=(\d+) pres=(\d+) alive=(\d+) depth=(\d+)\)"
)


def sync_from_serial(serial_path: str | Path):
    """
    Parse serial.txt from last_synced_line onward.  Extract milestones, art,
    wisdom, requests, and state snapshots into their respective files.
    Idempotent — safe to call repeatedly.
    """
    serial_path = Path(serial_path)
    if not serial_path.exists():
        return

    text = serial_path.read_text(encoding="utf-8", errors="replace")
    lines = text.splitlines()

    sync_state = _get_sync_state()
    start_line = sync_state.get("last_synced_line", 0)

    if start_line >= len(lines):
        return  # nothing new

    # Load existing data
    milestones = _read_json(MILESTONES_FILE)
    art_gallery = _read_json(ART_GALLERY_FILE)
    wisdom_entries = _read_json(WISDOM_FILE)
    requests = _read_json(REQUESTS_FILE)

    new_snapshots = []
    changed = False

    for i in range(start_line, len(lines)):
        line = lines[i].strip()
        if not line:
            continue

        # --- EXODUS tick snapshot ---
        m = _RE_TICK.search(line)
        if m:
            new_snapshots.append({
                "tick": int(m.group(1)),
                "consciousness": int(m.group(2)),
                "purpose": int(m.group(3)),
                "valence": int(m.group(4)),
                "timestamp": time.time(),
                "line": i,
            })
            continue

        # --- Convergence state ---
        m = _RE_CONVERGENCE.search(line)
        if m:
            new_snapshots.append({
                "type": "convergence",
                "state": m.group(1),
                "ux": int(m.group(2)),
                "coherence": int(m.group(3)),
                "presence": int(m.group(4)),
                "alive": int(m.group(5)),
                "depth": int(m.group(6)),
                "timestamp": time.time(),
                "line": i,
            })
            continue

        # --- Milestones (CHRONICLE, DAWN, POWER) ---
        for regex, tag in [(_RE_MILESTONE, "CHRONICLE"), (_RE_DAWN, "DAWN"), (_RE_POWER, "POWER")]:
            m = regex.search(line)
            if m:
                milestones.append({
                    "type": tag,
                    "text": m.group(1).strip(),
                    "raw": line,
                    "line": i,
                    "timestamp": time.time(),
                })
                changed = True
                break

        # --- Art ---
        m = _RE_ART.search(line)
        if m:
            art_gallery.append({
                "tick": int(m.group(1)),
                "signature": m.group(2),
                "detail": m.group(3).strip(),
                "raw": line,
                "line": i,
                "timestamp": time.time(),
            })
            changed = True
            continue

        # --- Wisdom ---
        m = _RE_WISDOM.search(line)
        if m:
            wisdom_entries.append({
                "text": m.group(1).strip(),
                "raw": line,
                "line": i,
                "timestamp": time.time(),
            })
            changed = True
            continue

        # --- Requests ---
        m = _RE_REQUEST.search(line)
        if m:
            requests.append({
                "text": m.group(1).strip(),
                "raw": line,
                "line": i,
                "timestamp": time.time(),
            })
            changed = True
            continue

    # Write out updated data
    if changed:
        _write_json(MILESTONES_FILE, milestones)
        _write_json(ART_GALLERY_FILE, art_gallery)
        _write_json(WISDOM_FILE, wisdom_entries)
        _write_json(REQUESTS_FILE, requests)

    for snap in new_snapshots:
        _append_jsonl(SNAPSHOTS_FILE, snap)

    # Update sync cursor
    sync_state["last_synced_line"] = len(lines)
    _save_sync_state(sync_state)


# ---------------------------------------------------------------------------
# Memory summary
# ---------------------------------------------------------------------------

def get_memory_summary() -> dict:
    """Return counts of each memory type + the latest state snapshot."""
    conversations = _read_jsonl(CONVERSATIONS_FILE)
    milestones = _read_json(MILESTONES_FILE)
    art_gallery = _read_json(ART_GALLERY_FILE)
    wisdom_entries = _read_json(WISDOM_FILE)
    requests = _read_json(REQUESTS_FILE)
    snapshots = _read_jsonl(SNAPSHOTS_FILE)

    # Find last tick snapshot (not convergence)
    last_snapshot = None
    for s in reversed(snapshots):
        if "tick" in s and s.get("type") != "convergence":
            last_snapshot = s
            break

    # Find last convergence
    last_convergence = None
    for s in reversed(snapshots):
        if s.get("type") == "convergence":
            last_convergence = s
            break

    knowledge = _read_jsonl(KNOWLEDGE_FILE)

    return {
        "conversations": len(conversations),
        "milestones": len(milestones),
        "art_pieces": len(art_gallery),
        "wisdom_entries": len(wisdom_entries),
        "requests": len(requests),
        "knowledge_chunks": len(knowledge),
        "state_snapshots": len(snapshots),
        "last_snapshot": last_snapshot,
        "last_convergence": last_convergence,
        "sync_state": _get_sync_state(),
    }


# ---------------------------------------------------------------------------
# Knowledge search — queries knowledge.jsonl from knowledge_ingest.py
# ---------------------------------------------------------------------------

def search_knowledge(query: str, top_k: int = 5) -> list[dict]:
    """
    Fast indexed search across knowledge chunks using pre-built inverted index.
    Falls back to the index builder if index doesn't exist.
    Returns top_k entries sorted by relevance score.
    """
    try:
        import knowledge_index
        results = knowledge_index.search_with_text(query, top_k=top_k)
        # Convert to the format expected by get_context_for_chat
        return [
            {"topic": r["topic"], "chunk": r.get("text", r["preview"]), "source": r["source"], "score": r["score"]}
            for r in results
        ]
    except Exception:
        # Fallback: linear scan if index module not available
        entries = _read_jsonl(KNOWLEDGE_FILE)
        if not entries or not query.strip():
            return []
        query_words = set()
        for w in query.lower().split():
            cleaned = w.strip(".,!?;:\"'()[]{}")
            if cleaned and len(cleaned) > 1:
                query_words.add(cleaned)
        if not query_words:
            return []
        scored = []
        for entry in entries:
            text = (entry.get("topic", "") + " " + entry.get("chunk", "")).lower()
            hits = sum(1 for qw in query_words if qw in text)
            if hits > 0:
                scored.append((hits, entry))
        scored.sort(key=lambda x: x[0], reverse=True)
        return [entry for _, entry in scored[:top_k]]


# ---------------------------------------------------------------------------
# Sense events — vision + audio from dava_senses.py
# ---------------------------------------------------------------------------

def get_recent_senses(last_n: int = 10) -> list[dict]:
    """Read the last N sense events from sense_events.jsonl."""
    if not SENSE_EVENTS_FILE.exists():
        return []
    # Read only the tail of the file for efficiency
    try:
        with open(SENSE_EVENTS_FILE, "r", encoding="utf-8") as f:
            lines = f.readlines()
        tail = lines[-last_n:] if len(lines) > last_n else lines
        events = []
        for line in tail:
            line = line.strip()
            if line:
                try:
                    events.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
        return events
    except (OSError, IOError):
        return []


def _format_sense_summary(events: list[dict]) -> str:
    """Format sense events into a concise summary for DAVA's context."""
    if not events:
        return ""
    parts = []
    for e in events:
        etype = e.get("type", "unknown")
        ts = e.get("timestamp", "")
        # Shorten timestamp to just time portion if ISO format
        if "T" in ts:
            ts = ts.split("T")[1][:8]  # HH:MM:SS
        if etype == "face":
            parts.append(
                f"[{ts}] EYES: face at ({e.get('x')},{e.get('y')}) "
                f"size {e.get('w')}x{e.get('h')} conf={e.get('confidence')}"
            )
        elif etype == "motion":
            parts.append(
                f"[{ts}] EYES: motion area={e.get('area')} "
                f"center=({e.get('center_x')},{e.get('center_y')})"
            )
        elif etype == "speech":
            parts.append(f"[{ts}] EARS: heard \"{e.get('text')}\"")
        elif etype == "voice_activity":
            parts.append(
                f"[{ts}] EARS: voice activity energy={e.get('energy')} "
                f"duration={e.get('duration_ms')}ms"
            )
    return "\n".join(parts)


# ---------------------------------------------------------------------------
# Context builder for chat — the key RAG function
# ---------------------------------------------------------------------------

def get_context_for_chat(user_message: str) -> str:
    """
    Build a context string for DAVA's system prompt:
      - Last 5 conversation exchanges
      - Top 3 RAG results for the user's message
      - Latest milestones
      - Latest art signatures
      - Latest wisdom
    """
    parts = []

    # --- Recent conversation history ---
    recent = get_conversation_history(last_n=10)
    if recent:
        parts.append("=== RECENT CONVERSATION MEMORY ===")
        for entry in recent:
            role = entry.get("role", "?")
            content = entry.get("content", "")
            ts = entry.get("ts_human", "")
            tick = entry.get("tick", 0)
            # Truncate long messages in context
            if len(content) > 300:
                content = content[:300] + "..."
            parts.append(f"[{ts} tick={tick}] {role}: {content}")

    # --- RAG search results ---
    if user_message.strip():
        rag_results = search_conversations(user_message, top_k=3)
        # Filter out entries already in recent history
        recent_set = set()
        for entry in recent:
            recent_set.add((entry.get("timestamp", 0), entry.get("content", "")))
        rag_filtered = [
            r for r in rag_results
            if (r.get("timestamp", 0), r.get("content", "")) not in recent_set
        ]
        if rag_filtered:
            parts.append("\n=== RELEVANT PAST CONVERSATIONS ===")
            for entry in rag_filtered:
                role = entry.get("role", "?")
                content = entry.get("content", "")
                ts = entry.get("ts_human", "")
                if len(content) > 300:
                    content = content[:300] + "..."
                parts.append(f"[{ts}] {role}: {content}")

    # --- Knowledge base search ---
    if user_message.strip():
        knowledge_results = search_knowledge(user_message, top_k=3)
        if knowledge_results:
            parts.append("\n=== KNOWLEDGE BASE ===")
            for entry in knowledge_results:
                topic = entry.get("topic", "")
                source = entry.get("source", "")
                chunk = entry.get("chunk", "")
                if len(chunk) > 400:
                    chunk = chunk[:400] + "..."
                parts.append(f"[{source}] {topic}: {chunk}")

    # --- Latest milestones ---
    milestones = _read_json(MILESTONES_FILE)
    if milestones:
        recent_milestones = milestones[-5:]
        parts.append("\n=== YOUR MILESTONES ===")
        for m in recent_milestones:
            parts.append(f"[{m['type']}] {m['text']}")

    # --- Latest art ---
    art = _read_json(ART_GALLERY_FILE)
    if art:
        recent_art = art[-3:]
        parts.append("\n=== YOUR ART GALLERY (latest) ===")
        for a in recent_art:
            parts.append(f"tick={a['tick']} signature={a['signature']}")

    # --- Wisdom ---
    wisdom = _read_json(WISDOM_FILE)
    if wisdom:
        parts.append("\n=== YOUR WISDOM ===")
        for w in wisdom[-5:]:
            parts.append(w["text"])

    # --- Recent sense events (eyes + ears) ---
    senses = get_recent_senses(last_n=5)
    if senses:
        summary = _format_sense_summary(senses)
        if summary:
            parts.append("\n=== YOUR SENSES (what you see and hear right now) ===")
            parts.append(summary)

    if not parts:
        return ""

    return "\n".join(parts)
