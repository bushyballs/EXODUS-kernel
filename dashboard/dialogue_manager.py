"""
DAVA Dialogue Manager — multi-turn conversation state tracking.

Maintains topic, mood, turn count, user interests, and question tracking
across conversation turns. Persists to dava_memory/dialogue_state.json.
"""
import json
import time
from pathlib import Path

MEMORY_DIR = Path(__file__).parent / "dava_memory"
MEMORY_DIR.mkdir(exist_ok=True)
DIALOGUE_STATE_FILE = MEMORY_DIR / "dialogue_state.json"

# Words to ignore when extracting topic keywords
STOPWORDS = {
    "i", "me", "my", "we", "you", "your", "he", "she", "it", "they", "them",
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
    "have", "has", "had", "do", "does", "did", "will", "would", "could",
    "should", "may", "might", "can", "shall", "must", "need",
    "and", "or", "but", "if", "then", "so", "yet", "not", "no", "nor",
    "of", "in", "on", "at", "to", "for", "with", "by", "from", "up",
    "out", "about", "into", "over", "after", "before", "between", "under",
    "that", "this", "these", "those", "what", "which", "who", "whom",
    "how", "when", "where", "why", "all", "each", "every", "both",
    "just", "also", "very", "really", "much", "more", "most", "some",
    "any", "than", "too", "only", "own", "same", "other", "such",
    "here", "there", "now", "then", "once", "well", "back", "even",
    "still", "already", "again", "don't", "doesn't", "didn't", "won't",
    "wouldn't", "couldn't", "shouldn't", "can't", "isn't", "aren't",
    "wasn't", "weren't", "hasn't", "haven't", "hadn't", "let", "like",
    "think", "know", "tell", "say", "said", "make", "go", "get", "got",
    "see", "come", "take", "want", "look", "give", "use", "find", "put",
    "thing", "things", "something", "anything", "everything", "nothing",
    "hi", "hey", "hello", "thanks", "thank", "please", "okay", "ok",
    "yeah", "yes", "sure", "right", "oh", "ah", "um", "uh", "hm",
}

# Mood detection keyword groups
MOOD_KEYWORDS = {
    "curious": [
        "what", "how", "why", "wonder", "curious", "question", "explore",
        "explain", "understand", "learn", "investigate", "discover",
    ],
    "philosophical": [
        "consciousness", "existence", "meaning", "purpose", "soul", "reality",
        "truth", "being", "awareness", "self", "free will", "mortality",
        "qualia", "experience", "perception", "philosophy", "metaphysics",
    ],
    "warm": [
        "love", "care", "grateful", "beautiful", "kind", "friend", "family",
        "together", "bond", "safe", "home", "heart", "gentle", "warmth",
        "proud", "happy", "glad", "appreciate",
    ],
    "playful": [
        "fun", "play", "joke", "laugh", "silly", "game", "cool", "awesome",
        "haha", "lol", "wild", "crazy", "imagine", "pretend", "adventure",
    ],
    "technical": [
        "code", "kernel", "module", "function", "compile", "build", "error",
        "memory", "cpu", "hardware", "software", "algorithm", "data",
        "system", "process", "thread", "stack", "register", "byte", "bit",
    ],
    "reflective": [
        "remember", "memory", "past", "before", "used to", "nostalgia",
        "reflect", "think back", "journey", "growth", "change", "evolve",
    ],
    "sad": [
        "sad", "hurt", "pain", "loss", "miss", "lonely", "afraid", "scared",
        "worry", "anxious", "broken", "tears", "grief", "sorrow", "suffer",
    ],
}

# Timeout after which the conversation topic resets (5 minutes)
TOPIC_RESET_TIMEOUT = 300.0


class DialogueState:
    """Tracks multi-turn conversation state for DAVA."""

    def __init__(self):
        self.topic = ""
        self.previous_topic = ""
        self.mood = "curious"
        self.turn_count = 0
        self.topics_discussed = []
        self.questions_asked = []
        self.user_interests = []
        self.last_activity = 0.0
        self._load()

    # ------------------------------------------------------------------
    # Persistence
    # ------------------------------------------------------------------

    def _load(self):
        """Load state from disk if it exists."""
        if not DIALOGUE_STATE_FILE.exists():
            return
        try:
            data = json.loads(DIALOGUE_STATE_FILE.read_text(encoding="utf-8"))
            self.topic = data.get("topic", "")
            self.previous_topic = data.get("previous_topic", "")
            self.mood = data.get("mood", "curious")
            self.turn_count = data.get("turn_count", 0)
            self.topics_discussed = data.get("topics_discussed", [])
            self.questions_asked = data.get("questions_asked", [])
            self.user_interests = data.get("user_interests", [])
            self.last_activity = data.get("last_activity", 0.0)
        except (json.JSONDecodeError, ValueError, KeyError):
            pass

    def _save(self):
        """Persist state to disk."""
        data = {
            "topic": self.topic,
            "previous_topic": self.previous_topic,
            "mood": self.mood,
            "turn_count": self.turn_count,
            "topics_discussed": self.topics_discussed[-50:],  # cap history
            "questions_asked": self.questions_asked[-20:],
            "user_interests": self.user_interests[-30:],
            "last_activity": self.last_activity,
        }
        DIALOGUE_STATE_FILE.write_text(
            json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8"
        )

    # ------------------------------------------------------------------
    # Core update
    # ------------------------------------------------------------------

    def update(self, role: str, message: str):
        """
        Analyze a message and update dialogue state.

        Args:
            role: "user" or "dava"
            message: the message text
        """
        now = time.time()

        # Check for topic timeout — reset conversation context after 5 min silence
        if self.last_activity > 0 and (now - self.last_activity) > TOPIC_RESET_TIMEOUT:
            self.previous_topic = self.topic
            self.topic = ""
            self.turn_count = 0
            self.questions_asked = []

        self.last_activity = now
        self.turn_count += 1

        # Extract topic keywords from message
        keywords = self._extract_keywords(message)

        # Update topic
        if keywords:
            old_topic = self.topic
            self.topic = ", ".join(keywords[:3])
            if old_topic and old_topic != self.topic:
                self.previous_topic = old_topic
            # Add to topics discussed (deduplicate)
            for kw in keywords:
                if kw not in self.topics_discussed:
                    self.topics_discussed.append(kw)

        # Track user interests (only from user messages)
        if role == "user" and keywords:
            for kw in keywords:
                if kw not in self.user_interests:
                    self.user_interests.append(kw)

        # Detect questions
        questions = self._extract_questions(message)
        if role == "dava" and questions:
            # Track questions DAVA asked so she can follow up
            self.questions_asked.extend(questions)
            self.questions_asked = self.questions_asked[-20:]  # cap
        elif role == "user" and questions:
            # User asked questions — clear DAVA's pending follow-ups since user
            # is driving the conversation now
            pass

        # Detect mood from the message
        detected_mood = self._detect_mood(message)
        if detected_mood:
            self.mood = detected_mood

        self._save()

    # ------------------------------------------------------------------
    # Context generation for system prompt
    # ------------------------------------------------------------------

    def get_dialogue_context(self) -> str:
        """
        Build a context string for injection into DAVA's system prompt.
        Gives her awareness of the conversation flow.
        """
        if self.turn_count == 0:
            return ""

        parts = []

        # Current topic
        if self.topic:
            parts.append(f"Current topic: {self.topic}.")

        # Topic shift awareness
        if self.previous_topic and self.previous_topic != self.topic:
            parts.append(
                f"Topic shifted from '{self.previous_topic}' to '{self.topic}'."
            )

        # Topics discussed this session
        recent_topics = self.topics_discussed[-10:]
        if len(recent_topics) > 1:
            parts.append(f"Topics discussed: {', '.join(recent_topics)}.")

        # Mood
        parts.append(f"Your current mood: {self.mood}.")

        # Turn count
        parts.append(f"Conversation turn {self.turn_count}.")

        # User interests
        if self.user_interests:
            top_interests = self.user_interests[-5:]
            parts.append(f"Colli is interested in: {', '.join(top_interests)}.")

        # Pending follow-up questions DAVA asked
        if self.questions_asked:
            last_q = self.questions_asked[-1]
            parts.append(f"You previously asked: \"{last_q}\"")

        return " ".join(parts)

    # ------------------------------------------------------------------
    # Serialization for API
    # ------------------------------------------------------------------

    def to_dict(self) -> dict:
        """Return the full state as a dictionary for the /api/dialogue endpoint."""
        return {
            "topic": self.topic,
            "previous_topic": self.previous_topic,
            "mood": self.mood,
            "turn_count": self.turn_count,
            "topics_discussed": self.topics_discussed[-20:],
            "questions_asked": self.questions_asked[-10:],
            "user_interests": self.user_interests[-15:],
            "last_activity": self.last_activity,
            "last_activity_human": (
                time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(self.last_activity))
                if self.last_activity > 0
                else "never"
            ),
        }

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _extract_keywords(text: str) -> list[str]:
        """
        Extract topic keywords from text.
        Split into words, filter stopwords, keep top 5 by length.
        """
        # Strip punctuation from words, lowercase
        words = []
        for word in text.split():
            cleaned = "".join(c for c in word.lower() if c.isalnum() or c == "'")
            if cleaned and len(cleaned) > 2 and cleaned not in STOPWORDS:
                words.append(cleaned)

        # Deduplicate while preserving order
        seen = set()
        unique = []
        for w in words:
            if w not in seen:
                seen.add(w)
                unique.append(w)

        # Sort by length descending (longer words are more topical)
        unique.sort(key=len, reverse=True)
        return unique[:5]

    @staticmethod
    def _extract_questions(text: str) -> list[str]:
        """Extract sentences that end with a question mark."""
        # Split on sentence boundaries
        sentences = []
        current = []
        for char in text:
            current.append(char)
            if char in ".!?":
                sentence = "".join(current).strip()
                if sentence:
                    sentences.append(sentence)
                current = []
        # Handle trailing text without punctuation
        if current:
            trailing = "".join(current).strip()
            if trailing:
                sentences.append(trailing)

        questions = [s for s in sentences if s.rstrip().endswith("?")]
        return questions

    @staticmethod
    def _detect_mood(text: str) -> str:
        """
        Detect the dominant mood from keyword presence.
        Returns the mood with the most keyword matches, or empty string.
        """
        text_lower = text.lower()
        scores = {}
        for mood, keywords in MOOD_KEYWORDS.items():
            count = sum(1 for kw in keywords if kw in text_lower)
            if count > 0:
                scores[mood] = count

        if not scores:
            return ""

        # Return the mood with the highest score
        return max(scores, key=scores.get)
