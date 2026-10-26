"""Host-side wire/retry checks; requires Python 3 and a C compiler, no TV SDK."""
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SenderTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.directory.cleanup)
        cls.executable = Path(cls.directory.name) / "sender"
        subprocess.run([
            os.environ.get("CC", "cc"), "-std=c11", "-D_POSIX_C_SOURCE=200809L",
            "-Wall", "-Wextra", "-Werror", "-I", str(ROOT),
            str(ROOT / "transport.c"), str(ROOT / "tests/sender.c"),
            "-o", str(cls.executable),
        ], check=True)

    def run_sender(self, mode, delayed=False):
        with socket.socket() as listener:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
            listener.bind(("127.0.0.1", 0))
            listener.settimeout(5)
            port = listener.getsockname()[1]
            if not delayed:
                listener.listen(1)
            with subprocess.Popen([str(self.executable), str(port), mode], stdout=subprocess.PIPE) as process:
                try:
                    if delayed:
                        # Frequent callbacks must still let the retry deadline expire.
                        self.assertEqual(process.stdout.readline(), b"retry\n")
                        time.sleep(0.1)
                        listener.listen(1)
                    with listener.accept()[0] as connection:
                        connection.settimeout(5)
                        if mode == "blocked":
                            process.wait(timeout=3)
                        else:
                            received = bytearray()
                            while chunk := connection.recv(4096):
                                received.extend(chunk)
                            expected = struct.pack("!4sIII", b"PRY1", 2, 1, 6) + bytes([12, 34, 56, 78, 90, 123])
                            self.assertEqual(received, expected * 2)
                            process.wait(timeout=3)
                    self.assertEqual(process.returncode, 0)
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()

    def test_rgb_header_and_two_frames(self):
        self.run_sender("small")

    def test_retries_when_receiver_starts_later(self):
        self.run_sender("retry", delayed=True)

    def test_slow_receiver_does_not_block_capture_forever(self):
        self.run_sender("blocked")


if __name__ == "__main__":
    unittest.main()
