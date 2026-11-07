"""PTY end-to-end: python3 check_commands.py <prysm-tui binary>.

Uses only temporary files and stays Off; no capture hardware or network is opened.
"""
import json
import os
from pathlib import Path
import sys
import tempfile
import termios
import time

from check_input import check


def main():
    assert len(sys.argv) == 2, __doc__
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        saved = root / "saved.json"
        candidate = root / "candidate with spaces.json"
        candidate.write_text(json.dumps({"processing": {"brightness_percent": 42}}))
        invalid = root / "invalid.json"
        invalid.write_text('{"active_profile":1}')

        def interact(master, read_for):
            os.write(master, f":load {candidate}\r".encode())
            deadline = time.monotonic() + 5
            while not saved.exists() and time.monotonic() < deadline:
                read_for(0.05)
            assert saved.exists(), f"typed load did not persist settings: {read_for(0)[-8000:]!r}"
            assert json.loads(saved.read_text())["processing"]["brightness_percent"] == 42
            before = saved.read_bytes()
            os.write(master, f":load {invalid}\r".encode())
            read_for(0.3)
            assert saved.read_bytes() == before, "invalid candidate changed persisted settings"
            os.write(master, b":off\r")
            read_for(0.3)

        check(termios.ECHO | termios.ECHONL, "ctrl-c",
              [sys.argv[1], str(saved)], interact)
        print("PASS: same-screen load, persistence, invalid replacement, Off and cleanup")


if __name__ == "__main__":
    main()
