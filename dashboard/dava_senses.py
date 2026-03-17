"""
DAVA Senses Daemon — Host-side vision and voice pipeline for DAVA.

Captures webcam + microphone input, runs ML detection, and writes events
to dava_memory/sense_events.jsonl for the dashboard server to inject
into DAVA's conversational context.

Usage:
    python dava_senses.py                    # all senses
    python dava_senses.py --no-video         # audio only
    python dava_senses.py --no-audio         # video only
    python dava_senses.py --whisper-model base  # use 'base' model instead of 'tiny'
    python dava_senses.py --energy-threshold 0.03  # voice activity threshold
    python dava_senses.py --motion-threshold 5000  # motion contour area threshold

Dependencies:
    pip install opencv-python sounddevice numpy
    pip install faster-whisper   # optional, for speech-to-text
"""

import argparse
import json
import os
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

# ---------------------------------------------------------------------------
# Graceful imports — every dependency is optional
# ---------------------------------------------------------------------------

_HAS_CV2 = False
_HAS_SOUNDDEVICE = False
_HAS_NUMPY = False
_HAS_FASTER_WHISPER = False
_HAS_WHISPER = False

try:
    import cv2
    _HAS_CV2 = True
except ImportError:
    cv2 = None

try:
    import numpy as np
    _HAS_NUMPY = True
except ImportError:
    np = None

try:
    import sounddevice as sd
    _HAS_SOUNDDEVICE = True
except (ImportError, OSError):
    sd = None

try:
    from faster_whisper import WhisperModel
    _HAS_FASTER_WHISPER = True
except ImportError:
    WhisperModel = None

if not _HAS_FASTER_WHISPER:
    try:
        import whisper as openai_whisper
        _HAS_WHISPER = True
    except ImportError:
        openai_whisper = None


# ---------------------------------------------------------------------------
# Paths and globals
# ---------------------------------------------------------------------------

MEMORY_DIR = Path(__file__).parent / "dava_memory"
MEMORY_DIR.mkdir(exist_ok=True)
SENSE_EVENTS_FILE = MEMORY_DIR / "sense_events.jsonl"

# Thread-safe write lock
_write_lock = threading.Lock()

# Graceful shutdown flag
_shutdown = threading.Event()


# ---------------------------------------------------------------------------
# Event writing
# ---------------------------------------------------------------------------

def write_event(event: dict):
    """Append a sense event to the JSONL file (thread-safe) and print to stdout."""
    event.setdefault("timestamp", datetime.now(timezone.utc).isoformat())
    with _write_lock:
        with open(SENSE_EVENTS_FILE, "a", encoding="utf-8") as f:
            f.write(json.dumps(event, ensure_ascii=False) + "\n")


def iso_now() -> str:
    return datetime.now(timezone.utc).isoformat()


# ---------------------------------------------------------------------------
# Vision thread
# ---------------------------------------------------------------------------

def vision_loop(args):
    """Webcam capture + face detection + motion detection at ~5 FPS."""
    if not _HAS_CV2:
        print("[DAVA_EYES] OpenCV not available — vision disabled")
        return
    if not _HAS_NUMPY:
        print("[DAVA_EYES] NumPy not available — vision disabled")
        return

    print("[DAVA_EYES] Initializing webcam...")

    cap = cv2.VideoCapture(0)
    if not cap.isOpened():
        print("[DAVA_EYES] No webcam found — vision disabled")
        return

    # Set a lower resolution for performance
    cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
    cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)

    print("[DAVA_EYES] Webcam active (640x480 @ ~5 FPS)")

    # Face detection — Haar cascade ships with OpenCV
    cascade_path = None
    # Try cv2.data first (pip-installed opencv-python)
    if hasattr(cv2, "data"):
        candidate = os.path.join(cv2.data.haarcascades, "haarcascade_frontalface_default.xml")
        if os.path.exists(candidate):
            cascade_path = candidate
    # Fallback: search common paths
    if cascade_path is None:
        for p in [
            "haarcascade_frontalface_default.xml",
            "/usr/share/opencv4/haarcascades/haarcascade_frontalface_default.xml",
            "/usr/share/opencv/haarcascades/haarcascade_frontalface_default.xml",
        ]:
            if os.path.exists(p):
                cascade_path = p
                break

    face_cascade = None
    if cascade_path:
        face_cascade = cv2.CascadeClassifier(cascade_path)
        if face_cascade.empty():
            face_cascade = None
            print("[DAVA_EYES] Face cascade failed to load — face detection disabled")
        else:
            print(f"[DAVA_EYES] Face detection active (Haar cascade)")
    else:
        print("[DAVA_EYES] Haar cascade XML not found — face detection disabled")

    # Background subtractor for motion detection
    bg_subtractor = cv2.createBackgroundSubtractorMOG2(
        history=300, varThreshold=50, detectShadows=False
    )

    motion_threshold = args.motion_threshold
    frame_interval = 0.2  # 5 FPS

    print(f"[DAVA_EYES] Motion threshold: {motion_threshold} px^2")
    print("[DAVA_EYES] Vision loop running")

    last_face_time = 0
    last_motion_time = 0
    face_cooldown = 1.0     # min seconds between face events
    motion_cooldown = 2.0   # min seconds between motion events

    while not _shutdown.is_set():
        start = time.monotonic()

        ret, frame = cap.read()
        if not ret:
            time.sleep(0.5)
            continue

        gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)
        now = time.monotonic()

        # --- Face detection ---
        if face_cascade is not None and (now - last_face_time) >= face_cooldown:
            faces = face_cascade.detectMultiScale(
                gray,
                scaleFactor=1.2,
                minNeighbors=5,
                minSize=(40, 40),
            )
            if len(faces) > 0:
                last_face_time = now
                for (x, y, w, h) in faces:
                    # Confidence approximation: larger face = higher confidence
                    conf = min(1.0, (w * h) / (640 * 480) * 10)
                    event = {
                        "type": "face",
                        "x": int(x),
                        "y": int(y),
                        "w": int(w),
                        "h": int(h),
                        "confidence": round(conf, 3),
                        "timestamp": iso_now(),
                        "tick": 0,
                    }
                    write_event(event)
                    print(f"[DAVA_EYES] face detected at ({x},{y}) size {w}x{h}")

        # --- Motion detection ---
        if (now - last_motion_time) >= motion_cooldown:
            fg_mask = bg_subtractor.apply(frame)
            # Threshold to remove shadows/noise
            _, thresh = cv2.threshold(fg_mask, 200, 255, cv2.THRESH_BINARY)
            contours, _ = cv2.findContours(thresh, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)

            total_area = 0
            cx_sum, cy_sum, count = 0, 0, 0
            for cnt in contours:
                area = cv2.contourArea(cnt)
                if area > 500:  # ignore tiny noise
                    total_area += area
                    M = cv2.moments(cnt)
                    if M["m00"] > 0:
                        cx_sum += int(M["m10"] / M["m00"])
                        cy_sum += int(M["m01"] / M["m00"])
                        count += 1

            if total_area > motion_threshold and count > 0:
                last_motion_time = now
                event = {
                    "type": "motion",
                    "area": int(total_area),
                    "center_x": cx_sum // count,
                    "center_y": cy_sum // count,
                    "timestamp": iso_now(),
                }
                write_event(event)
                print(f"[DAVA_EYES] motion detected area={total_area} center=({cx_sum // count},{cy_sum // count})")

        # Maintain ~5 FPS
        elapsed = time.monotonic() - start
        sleep_time = frame_interval - elapsed
        if sleep_time > 0:
            _shutdown.wait(sleep_time)

    cap.release()
    print("[DAVA_EYES] Vision loop stopped")


# ---------------------------------------------------------------------------
# Audio thread
# ---------------------------------------------------------------------------

def audio_loop(args):
    """Microphone capture with VAD and optional STT."""
    if not _HAS_SOUNDDEVICE:
        print("[DAVA_EARS] sounddevice not available — audio disabled")
        return
    if not _HAS_NUMPY:
        print("[DAVA_EARS] NumPy not available — audio disabled")
        return

    print("[DAVA_EARS] Initializing microphone...")

    # Check that a microphone is actually available
    try:
        device_info = sd.query_devices(kind="input")
        print(f"[DAVA_EARS] Input device: {device_info['name']}")
    except Exception as e:
        print(f"[DAVA_EARS] No microphone found ({e}) — audio disabled")
        return

    # --- Optional STT setup ---
    whisper_model = None
    stt_available = False

    if _HAS_FASTER_WHISPER:
        model_name = args.whisper_model
        print(f"[DAVA_EARS] Loading faster-whisper model '{model_name}'...")
        try:
            whisper_model = WhisperModel(model_name, device="cpu", compute_type="int8")
            stt_available = True
            print(f"[DAVA_EARS] STT active (faster-whisper, model={model_name})")
        except Exception as e:
            print(f"[DAVA_EARS] faster-whisper failed to load ({e}) — STT disabled")
    elif _HAS_WHISPER:
        model_name = args.whisper_model
        print(f"[DAVA_EARS] Loading openai-whisper model '{model_name}'...")
        try:
            whisper_model = openai_whisper.load_model(model_name)
            stt_available = True
            print(f"[DAVA_EARS] STT active (openai-whisper, model={model_name})")
        except Exception as e:
            print(f"[DAVA_EARS] openai-whisper failed to load ({e}) — STT disabled")
    else:
        print("[DAVA_EARS] No whisper library found — VAD only, no transcription")

    # Audio parameters
    sample_rate = 16000
    chunk_duration = 0.5  # seconds per chunk
    chunk_samples = int(sample_rate * chunk_duration)
    energy_threshold = args.energy_threshold

    # Speech accumulation
    speech_buffer = []
    speech_active = False
    silence_chunks = 0
    silence_needed = 4  # chunks of silence to end speech segment (~2s)
    min_speech_chunks = 2  # minimum chunks to consider a speech segment (~1s)

    print(f"[DAVA_EARS] Energy threshold: {energy_threshold}")
    print("[DAVA_EARS] Audio loop running")

    last_vad_time = 0
    vad_cooldown = 3.0  # min seconds between VAD-only events

    def audio_callback(indata, frames, time_info, status):
        nonlocal speech_buffer, speech_active, silence_chunks, last_vad_time

        if status:
            pass  # ignore overflows

        audio_chunk = indata[:, 0].copy()  # mono

        # Compute RMS energy
        rms = float(np.sqrt(np.mean(audio_chunk ** 2)))

        if rms > energy_threshold:
            # Voice activity detected
            speech_active = True
            silence_chunks = 0
            speech_buffer.append(audio_chunk)
        elif speech_active:
            silence_chunks += 1
            speech_buffer.append(audio_chunk)

            if silence_chunks >= silence_needed:
                # End of speech segment
                speech_active = False
                silence_chunks = 0

                if len(speech_buffer) >= min_speech_chunks:
                    segment = np.concatenate(speech_buffer)
                    duration_ms = int(len(segment) / sample_rate * 1000)

                    if stt_available and whisper_model is not None:
                        _transcribe_segment(
                            segment, sample_rate, duration_ms, whisper_model
                        )
                    else:
                        now = time.monotonic()
                        if (now - last_vad_time) >= vad_cooldown:
                            last_vad_time = now
                            seg_rms = float(np.sqrt(np.mean(segment ** 2)))
                            event = {
                                "type": "voice_activity",
                                "energy": round(seg_rms, 4),
                                "duration_ms": duration_ms,
                                "timestamp": iso_now(),
                            }
                            write_event(event)
                            print(
                                f"[DAVA_EARS] voice activity "
                                f"(energy={seg_rms:.4f}, duration={duration_ms}ms)"
                            )

                speech_buffer = []

    try:
        with sd.InputStream(
            samplerate=sample_rate,
            channels=1,
            dtype="float32",
            blocksize=chunk_samples,
            callback=audio_callback,
        ):
            while not _shutdown.is_set():
                _shutdown.wait(0.5)
    except Exception as e:
        print(f"[DAVA_EARS] Audio stream error: {e}")

    print("[DAVA_EARS] Audio loop stopped")


def _transcribe_segment(audio_data, sample_rate, duration_ms, model):
    """Run whisper STT on a speech segment."""
    try:
        if _HAS_FASTER_WHISPER:
            # faster-whisper expects numpy array
            segments, info = model.transcribe(
                audio_data,
                beam_size=1,
                language="en",
                vad_filter=True,
            )
            text_parts = []
            for seg in segments:
                text_parts.append(seg.text.strip())
            text = " ".join(text_parts).strip()
            confidence = round(info.language_probability, 3) if hasattr(info, "language_probability") else 0.0

        elif _HAS_WHISPER:
            # openai-whisper
            import tempfile
            import wave

            # Write to temp WAV for openai-whisper
            with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
                tmp_path = tmp.name
                with wave.open(tmp, "wb") as wf:
                    wf.setnchannels(1)
                    wf.setsampwidth(2)  # 16-bit
                    wf.setframerate(sample_rate)
                    wf.writeframes((audio_data * 32767).astype(np.int16).tobytes())

            result = model.transcribe(tmp_path, fp16=False, language="en")
            text = result.get("text", "").strip()
            confidence = 0.0
            # Try to get average segment confidence
            segs = result.get("segments", [])
            if segs:
                avg = sum(s.get("avg_logprob", -1.0) for s in segs) / len(segs)
                # Convert log-prob to rough 0-1 confidence
                import math
                confidence = round(min(1.0, max(0.0, math.exp(avg))), 3)

            try:
                os.unlink(tmp_path)
            except OSError:
                pass
        else:
            return

        if text:
            event = {
                "type": "speech",
                "text": text,
                "confidence": confidence,
                "duration_ms": duration_ms,
                "timestamp": iso_now(),
            }
            write_event(event)
            print(f'[DAVA_EARS] speech: "{text}"')
        else:
            # Transcription returned nothing — log as voice activity
            seg_rms = float(np.sqrt(np.mean(audio_data ** 2)))
            event = {
                "type": "voice_activity",
                "energy": round(seg_rms, 4),
                "duration_ms": duration_ms,
                "timestamp": iso_now(),
            }
            write_event(event)
            print(f"[DAVA_EARS] voice activity (energy={seg_rms:.4f}, duration={duration_ms}ms)")

    except Exception as e:
        print(f"[DAVA_EARS] STT error: {e}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def print_banner(args):
    """Print startup banner showing which senses are active."""
    print()
    print("=" * 60)
    print("  DAVA SENSES DAEMON")
    print("  Host-side vision + voice pipeline for Exodus kernel")
    print("=" * 60)
    print()

    # Vision status
    if args.no_video:
        print("  [EYES]  DISABLED (--no-video)")
    elif not _HAS_CV2:
        print("  [EYES]  UNAVAILABLE (pip install opencv-python)")
    else:
        print("  [EYES]  ACTIVE — face detection + motion tracking")

    # Audio status
    if args.no_audio:
        print("  [EARS]  DISABLED (--no-audio)")
    elif not _HAS_SOUNDDEVICE:
        print("  [EARS]  UNAVAILABLE (pip install sounddevice)")
    else:
        if _HAS_FASTER_WHISPER or _HAS_WHISPER:
            lib = "faster-whisper" if _HAS_FASTER_WHISPER else "openai-whisper"
            print(f"  [EARS]  ACTIVE — VAD + STT ({lib}, model={args.whisper_model})")
        else:
            print("  [EARS]  ACTIVE — VAD only (pip install faster-whisper for STT)")

    print()
    print(f"  Events file: {SENSE_EVENTS_FILE}")
    print(f"  Energy threshold: {args.energy_threshold}")
    print(f"  Motion threshold: {args.motion_threshold} px^2")
    print()
    print("  Press Ctrl+C to stop")
    print("=" * 60)
    print()


def main():
    parser = argparse.ArgumentParser(description="DAVA Senses Daemon")
    parser.add_argument(
        "--no-video", action="store_true",
        help="Disable webcam/vision capture"
    )
    parser.add_argument(
        "--no-audio", action="store_true",
        help="Disable microphone/audio capture"
    )
    parser.add_argument(
        "--whisper-model", default="tiny",
        help="Whisper model size: tiny, base, small, medium, large (default: tiny)"
    )
    parser.add_argument(
        "--energy-threshold", type=float, default=0.02,
        help="RMS energy threshold for voice activity detection (default: 0.02)"
    )
    parser.add_argument(
        "--motion-threshold", type=int, default=5000,
        help="Minimum contour area (px^2) for motion events (default: 5000)"
    )
    args = parser.parse_args()

    print_banner(args)

    threads = []

    # Start vision thread
    if not args.no_video and _HAS_CV2 and _HAS_NUMPY:
        t = threading.Thread(target=vision_loop, args=(args,), name="DAVA_EYES", daemon=True)
        t.start()
        threads.append(t)
    elif not args.no_video:
        if not _HAS_CV2:
            print("[DAVA_EYES] Skipped — opencv-python not installed")
        if not _HAS_NUMPY:
            print("[DAVA_EYES] Skipped — numpy not installed")

    # Start audio thread
    if not args.no_audio and _HAS_SOUNDDEVICE and _HAS_NUMPY:
        t = threading.Thread(target=audio_loop, args=(args,), name="DAVA_EARS", daemon=True)
        t.start()
        threads.append(t)
    elif not args.no_audio:
        if not _HAS_SOUNDDEVICE:
            print("[DAVA_EARS] Skipped — sounddevice not installed")
        if not _HAS_NUMPY:
            print("[DAVA_EARS] Skipped — numpy not installed")

    if not threads:
        print("\n[DAVA SENSES] No senses available. Install dependencies or check hardware.")
        print("  pip install opencv-python sounddevice numpy")
        sys.exit(1)

    print(f"[DAVA SENSES] {len(threads)} sense thread(s) running\n")

    try:
        while True:
            time.sleep(1.0)
    except KeyboardInterrupt:
        print("\n[DAVA SENSES] Shutting down...")
        _shutdown.set()
        for t in threads:
            t.join(timeout=3.0)
        print("[DAVA SENSES] Stopped")


if __name__ == "__main__":
    main()
