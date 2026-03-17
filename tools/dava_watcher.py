"""
DAVA Disk Persistence Watcher
Tails the QEMU serial log and saves [DAVA_SAVE] entries as real .rs files.

Usage:
    python tools/dava_watcher.py                  # watches serial.txt in cwd
    python tools/dava_watcher.py path/to/serial.txt
    python tools/dava_watcher.py --output-dir C:\Users\colli\exodus\dava_output

The kernel emits lines like:
    [DAVA_SAVE] dava_truth.rs :: pub const TRUTH_MAX: u32 = 1000;

This script:
  1. Tails serial.txt for new [DAVA_SAVE] lines
  2. Appends the content to the named .rs file in the output directory
  3. Logs each write to console
"""
import os
import sys
import time
import argparse
from pathlib import Path
from datetime import datetime


def parse_dava_line(line: str):
    marker = "[DAVA_SAVE] "
    idx = line.find(marker)
    if idx == -1:
        return None
    rest = line[idx + len(marker):].strip()
    parts = rest.split(" :: ", 1)
    if len(parts) != 2:
        return None
    filename = parts[0].strip()
    content = parts[1].strip()
    if not filename or not content:
        return None
    return filename, content


def watch(serial_path: Path, output_dir: Path):
    output_dir.mkdir(parents=True, exist_ok=True)
    print(f"[dava_watcher] Watching: {serial_path}")
    print(f"[dava_watcher] Output:   {output_dir}")
    print(f"[dava_watcher] Waiting for [DAVA_SAVE] lines...")

    saved_count = 0
    total_bytes = 0

    # Start at end of file
    try:
        with open(serial_path, "r", encoding="utf-8", errors="replace") as f:
            f.seek(0, 2)  # seek to end
            while True:
                line = f.readline()
                if not line:
                    time.sleep(0.5)
                    continue

                parsed = parse_dava_line(line)
                if parsed is None:
                    # Also print flush notifications
                    if "[DAVA_FLUSH]" in line:
                        print(f"  {line.strip()}")
                    continue

                filename, content = parsed
                filepath = output_dir / filename

                # Append content to file (accumulates over time)
                with open(filepath, "a", encoding="utf-8") as out:
                    out.write(content + "\n")

                saved_count += 1
                total_bytes += len(content)
                ts = datetime.now().strftime("%H:%M:%S")
                print(f"  [{ts}] #{saved_count} -> {filename} (+{len(content)}B, total {total_bytes}B)")

    except KeyboardInterrupt:
        print(f"\n[dava_watcher] Stopped. Saved {saved_count} improvements ({total_bytes} bytes)")
    except FileNotFoundError:
        print(f"[dava_watcher] ERROR: {serial_path} not found. Start QEMU first.")
        sys.exit(1)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="DAVA Disk Persistence Watcher")
    parser.add_argument("serial", nargs="?", default="serial.txt",
                        help="Path to QEMU serial log (default: serial.txt)")
    parser.add_argument("--output-dir", "-o", default="dava_output",
                        help="Directory to write .rs files (default: dava_output)")
    args = parser.parse_args()

    serial_path = Path(args.serial)
    output_dir = Path(args.output_dir)

    watch(serial_path, output_dir)
