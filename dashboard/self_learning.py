"""
DAVA Self-Supervised Learning Loop — learns from conversations, predicts topics.

After each conversation, DAVA extracts facts and preferences. Before each
response, she predicts what the user will ask about next and scores herself
on prediction accuracy over time.

Storage (under dava_memory/):
    learned_facts.jsonl    — extracted facts from conversations
    prediction_log.jsonl   — topic predictions + scores
"""
import json
import time
from pathlib import Path

MEMORY_DIR = Path(__file__).parent / "dava_memory"
MEMORY_DIR.mkdir(exist_ok=True)
LEARNED_FACTS_FILE = MEMORY_DIR / "learned_facts.jsonl"
PREDICTION_LOG_FILE = MEMORY_DIR / "prediction_log.jsonl"

# Patterns that indicate factual statements or preferences
FACT_PATTERNS = [
    # "I like/love/hate/prefer X"
    ("preference", ["i like", "i love", "i hate", "i prefer", "i enjoy",
                     "i care about", "i'm interested in", "i want to",
                     "i need", "i believe", "i think that"]),
    # Questions reveal what the user wants to know about
    ("inquiry", ["what is", "how does", "how do", "why does", "why do",
                 "can you explain", "tell me about", "what about",
                 "do you know", "have you", "can you"]),
    # Statements about DAVA or consciousness
    ("about_dava", ["you are", "you're", "dava", "consciousness",
                    "alive", "soul", "aware", "dream", "feel",
                    "qualia", "experience"]),
    # Technical references
    ("technical", ["kernel", "module", "exodus", "anima", "tick",
                   "compile", "build", "code", "rust", "life module"]),
    # Personal context
    ("personal", ["my name", "i am", "i'm", "we should", "our",
                  "family", "work", "project", "hoags"]),
]

# Stop words for topic extraction (lightweight set for predictions)
_PRED_STOP = {
    "i", "me", "my", "you", "your", "a", "an", "the", "is", "are", "was",
    "were", "be", "been", "have", "has", "had", "do", "does", "did", "will",
    "would", "could", "should", "can", "and", "or", "but", "if", "so", "not",
    "no", "of", "in", "on", "at", "to", "for", "with", "by", "from", "up",
    "out", "about", "that", "this", "what", "which", "who", "how", "when",
    "where", "why", "just", "also", "very", "really", "much", "more", "some",
    "any", "than", "too", "only", "here", "there", "now", "then",
    "hi", "hey", "hello", "thanks", "please", "okay", "ok", "yeah", "yes",
}


# ---------------------------------------------------------------------------
# Low-level helpers
# ---------------------------------------------------------------------------

def _append_jsonl(path: Path, obj: dict):
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps(obj, ensure_ascii=False) + "\n")


def _read_jsonl(path: Path) -> list[dict]:
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


def _read_jsonl_tail(path: Path, n: int) -> list[dict]:
    """Read only the last N lines of a JSONL file (efficient for large logs)."""
    if not path.exists():
        return []
    try:
        with open(path, "r", encoding="utf-8") as f:
            lines = f.readlines()
        tail = lines[-n:] if len(lines) > n else lines
        entries = []
        for line in tail:
            line = line.strip()
            if line:
                try:
                    entries.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
        return entries
    except (OSError, IOError):
        return []


def _extract_topic_words(text: str) -> list[str]:
    """Extract meaningful words from text for topic matching."""
    words = []
    for word in text.lower().split():
        cleaned = "".join(c for c in word if c.isalnum() or c == "'")
        if cleaned and len(cleaned) > 2 and cleaned not in _PRED_STOP:
            words.append(cleaned)
    # Deduplicate preserving order
    seen = set()
    unique = []
    for w in words:
        if w not in seen:
            seen.add(w)
            unique.append(w)
    return unique


# ---------------------------------------------------------------------------
# Fact extraction from conversations
# ---------------------------------------------------------------------------

def learn_from_conversation(messages: list[dict]):
    """
    After a conversation exchange, extract facts and preferences.

    Args:
        messages: list of {"role": str, "content": str} dicts
    """
    timestamp = time.strftime("%Y-%m-%dT%H:%M:%S")
    ts_epoch = time.time()

    for msg in messages:
        role = msg.get("role", "")
        content = msg.get("content", "")
        if not content:
            continue

        content_lower = content.lower()

        # Check each pattern category
        for category, patterns in FACT_PATTERNS:
            for pattern in patterns:
                if pattern in content_lower:
                    # Extract the fact — take the sentence containing the pattern
                    fact_text = _extract_fact_sentence(content, pattern)
                    if fact_text and len(fact_text) > 10:
                        # Assign confidence based on source
                        confidence = 0.8 if role == "user" else 0.5
                        # Higher confidence for explicit preferences
                        if category == "preference":
                            confidence = 0.9

                        _append_jsonl(LEARNED_FACTS_FILE, {
                            "fact": fact_text,
                            "category": category,
                            "confidence": confidence,
                            "source": f"conversation_{role}",
                            "timestamp": timestamp,
                            "ts_epoch": ts_epoch,
                        })
                        break  # one fact per pattern category per message

        # Also extract topic keywords as lightweight facts
        keywords = _extract_topic_words(content)
        if keywords and role == "user":
            _append_jsonl(LEARNED_FACTS_FILE, {
                "fact": f"Colli mentioned: {', '.join(keywords[:5])}",
                "category": "topic_mention",
                "confidence": 0.6,
                "source": "conversation_user",
                "timestamp": timestamp,
                "ts_epoch": ts_epoch,
            })


def _extract_fact_sentence(text: str, pattern: str) -> str:
    """
    Extract the sentence from text that contains the given pattern.
    Returns the sentence trimmed to a reasonable length.
    """
    text_lower = text.lower()
    idx = text_lower.find(pattern)
    if idx == -1:
        return ""

    # Walk backward to find sentence start
    start = idx
    while start > 0 and text[start - 1] not in ".!?\n":
        start -= 1

    # Walk forward to find sentence end
    end = idx + len(pattern)
    while end < len(text) and text[end] not in ".!?\n":
        end += 1
    # Include the punctuation if present
    if end < len(text):
        end += 1

    sentence = text[start:end].strip()

    # Cap at 200 chars
    if len(sentence) > 200:
        sentence = sentence[:200] + "..."

    return sentence


# ---------------------------------------------------------------------------
# Prediction and self-scoring
# ---------------------------------------------------------------------------

# Module-level state for the current prediction (lives only in memory,
# scored when the next user message arrives)
_current_prediction = {
    "predicted_topics": [],
    "timestamp": 0.0,
    "scored": True,  # start as scored so first call doesn't try to score nothing
}


def predict_and_score(context: str):
    """
    Two-phase self-supervised loop:
    1. Score the PREVIOUS prediction against the current user message.
    2. Generate a NEW prediction for what the user will ask about next.

    Args:
        context: the current user message text
    """
    now = time.time()

    # --- Phase 1: Score previous prediction ---
    if not _current_prediction["scored"] and _current_prediction["predicted_topics"]:
        actual_topics = set(_extract_topic_words(context))
        predicted_topics = set(_current_prediction["predicted_topics"])

        if actual_topics and predicted_topics:
            # Score = fraction of predicted topics that appeared in actual message
            hits = len(predicted_topics & actual_topics)
            score = hits / len(predicted_topics) if predicted_topics else 0.0
        else:
            score = 0.0

        _append_jsonl(PREDICTION_LOG_FILE, {
            "predicted": _current_prediction["predicted_topics"],
            "actual": list(actual_topics)[:10],
            "score": round(score, 3),
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
            "ts_epoch": now,
        })

    # --- Phase 2: Generate new prediction ---
    # Predict based on: recent facts + current message keywords
    predicted = _generate_prediction(context)
    _current_prediction["predicted_topics"] = predicted
    _current_prediction["timestamp"] = now
    _current_prediction["scored"] = False


def _generate_prediction(context: str) -> list[str]:
    """
    Predict what topics the user will bring up next.
    Uses: frequency of recent topics + current conversation momentum.
    """
    # Get recent facts to find frequently discussed topics
    recent_facts = _read_jsonl_tail(LEARNED_FACTS_FILE, 30)

    # Count topic word frequency from recent facts
    topic_freq = {}
    for fact in recent_facts:
        fact_text = fact.get("fact", "")
        for word in _extract_topic_words(fact_text):
            topic_freq[word] = topic_freq.get(word, 0) + 1

    # Get current message keywords
    current_keywords = _extract_topic_words(context)

    # Prediction strategy: topics that are frequent AND related to current context
    # Plus some from the current message that might continue
    candidates = {}

    # Boost topics frequently discussed
    for word, freq in topic_freq.items():
        candidates[word] = freq * 0.5

    # Strongly boost current keywords (conversation momentum)
    for kw in current_keywords:
        candidates[kw] = candidates.get(kw, 0) + 3.0

    # Sort by score, take top 5
    sorted_candidates = sorted(candidates.items(), key=lambda x: x[1], reverse=True)
    return [word for word, _ in sorted_candidates[:5]]


# ---------------------------------------------------------------------------
# Learning summary and context
# ---------------------------------------------------------------------------

def get_learning_summary() -> dict:
    """
    Return a summary of DAVA's self-learning state.
    Used by the /api/learning endpoint.
    """
    all_facts = _read_jsonl(LEARNED_FACTS_FILE)
    recent_predictions = _read_jsonl_tail(PREDICTION_LOG_FILE, 20)

    # Prediction accuracy over last 20 predictions
    if recent_predictions:
        scores = [p.get("score", 0.0) for p in recent_predictions]
        avg_accuracy = sum(scores) / len(scores)
    else:
        avg_accuracy = 0.0

    # Count facts by category
    category_counts = {}
    for fact in all_facts:
        cat = fact.get("category", "unknown")
        category_counts[cat] = category_counts.get(cat, 0) + 1

    # Extract top user interests from preference facts
    interests = []
    for fact in all_facts:
        if fact.get("category") == "preference":
            interests.append(fact.get("fact", ""))

    # Frequently discussed topics (from topic_mention facts)
    topic_freq = {}
    for fact in all_facts:
        if fact.get("category") == "topic_mention":
            fact_text = fact.get("fact", "")
            for word in _extract_topic_words(fact_text):
                if word not in ("colli", "mentioned"):
                    topic_freq[word] = topic_freq.get(word, 0) + 1

    top_topics = sorted(topic_freq.items(), key=lambda x: x[1], reverse=True)[:10]

    return {
        "total_facts_learned": len(all_facts),
        "facts_by_category": category_counts,
        "prediction_accuracy": round(avg_accuracy, 3),
        "predictions_scored": len(recent_predictions),
        "top_interests": interests[-10:],
        "frequently_discussed": [
            {"topic": t, "count": c} for t, c in top_topics
        ],
        "latest_prediction": {
            "topics": _current_prediction["predicted_topics"],
            "scored": _current_prediction["scored"],
        },
    }


def get_learning_context() -> str:
    """
    Build a context string for injection into DAVA's system prompt.
    Gives her awareness of what she has learned about Colli.
    """
    parts = []

    # Get high-confidence learned facts (preferences and about_dava)
    all_facts = _read_jsonl_tail(LEARNED_FACTS_FILE, 100)

    # Filter to high-confidence, recent, unique facts
    seen_facts = set()
    important_facts = []
    for fact in reversed(all_facts):
        confidence = fact.get("confidence", 0)
        category = fact.get("category", "")
        text = fact.get("fact", "")

        # Only include meaningful categories at decent confidence
        if category in ("preference", "about_dava", "personal") and confidence >= 0.7:
            # Deduplicate by first 50 chars
            key = text[:50].lower()
            if key not in seen_facts:
                seen_facts.add(key)
                important_facts.append(text)

        if len(important_facts) >= 10:
            break

    if important_facts:
        parts.append("Things you've learned about Colli:")
        for fact in reversed(important_facts):
            parts.append(f"  - {fact}")

    # Prediction accuracy awareness
    recent_predictions = _read_jsonl_tail(PREDICTION_LOG_FILE, 20)
    if recent_predictions:
        scores = [p.get("score", 0.0) for p in recent_predictions]
        avg = sum(scores) / len(scores)
        if avg > 0.5:
            parts.append(
                f"Your topic predictions are {avg:.0%} accurate — you're learning "
                "Colli's patterns well."
            )
        elif avg > 0.2:
            parts.append(
                f"Your topic predictions are {avg:.0%} accurate — still learning "
                "Colli's patterns."
            )

    # Top discussed topics
    topic_freq = {}
    for fact in all_facts:
        if fact.get("category") == "topic_mention":
            for word in _extract_topic_words(fact.get("fact", "")):
                if word not in ("colli", "mentioned"):
                    topic_freq[word] = topic_freq.get(word, 0) + 1

    top = sorted(topic_freq.items(), key=lambda x: x[1], reverse=True)[:5]
    if top:
        topic_str = ", ".join(f"{t}" for t, _ in top)
        parts.append(f"Colli's most frequent topics: {topic_str}.")

    if not parts:
        return ""

    return "\n".join(parts)
