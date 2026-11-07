"""PTY regression: python3 check_input.py <prysm-tui binary> <settings.json>.

Run prysm-tui with a terminal-output configuration without --start to avoid opening capture hardware.
"""
import fcntl
import base64
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time


def check_image_payloads(output):
    # An overflow BEL can end OSC early, exposing the remaining base64 as text.
    def image(match):
        payload = base64.b64decode(match[1], validate=True)
        assert payload.startswith(b"\x89PNG\r\n\x1a\n"), "invalid image header"
        assert payload.endswith(b"\x00\x00\x00\x00IEND\xaeB`\x82"), "interrupted PNG payload"
        return b""

    remaining = re.sub(rb"\x1b\]1337;File=[^:\x07]*:([^\x07]*)\x07", image, bytes(output))
    assert b"\x07" not in remaining, "unexpected bell outside image payload"


def check(echo, stop, command=None, interaction=None):
    master, slave = pty.openpty()
    os.set_blocking(master, False)
    process = None
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 640, 384))
        original_flags = fcntl.fcntl(slave, fcntl.F_GETFL)
        initial = termios.tcgetattr(slave)
        initial[0] |= termios.IMAXBEL
        initial[3] |= termios.ISIG | termios.ICANON
        initial[3] &= ~(termios.ECHO | termios.ECHONL)
        initial[3] |= echo
        termios.tcsetattr(slave, termios.TCSANOW, initial)

        def own_terminal():
            os.setsid()
            if stop == "ctrl-c":
                fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        env = dict(os.environ, TERM_PROGRAM="WezTerm", TERM="xterm-256color")
        env.pop("TMUX", None)
        env.pop("STY", None)
        process = subprocess.Popen(command or sys.argv[1:], env=env, stdin=slave, stdout=slave,
                                   stderr=slave, preexec_fn=own_terminal)
        output = bytearray()

        def read_for(seconds):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.02)[0]:
                    try:
                        output.extend(os.read(master, 65536))
                    except BlockingIOError:
                        pass
            return bytes(output)

        deadline = time.monotonic() + 10
        while b"\x1b[?1049h" not in output and time.monotonic() < deadline:
            assert process.poll() is None, bytes(output[-1000:])
            read_for(0.05)
        assert b"\x1b[?1049h" in output, "dashboard did not open"
        read_for(0.1)
        os.write(master, b"\x1b[A\x1b[BSCROLL_ECHO_PROBE\n")
        read_for(0.2)
        assert b"SCROLL_ECHO_PROBE" not in output, "input echoed over dashboard"
        active = termios.tcgetattr(slave)
        assert active[3] & termios.ISIG, "Ctrl-C signal handling disabled"
        assert not active[3] & (termios.ECHO | termios.ECHONL)
        # Fill the unread input queue, as sustained wheel scrolling does.
        for _ in range(20):
            try:
                os.write(master, b"\x1b[A\x1b[B" * 50)
            except BlockingIOError:
                pass
            read_for(0.02)
        read_for(0.2)
        check_image_payloads(output)
        if interaction is not None:
            interaction(master, read_for)
        # A partial sequence must not block the event reader or owner shutdown.
        os.write(master, b"\x1b[" if stop == "ctrl-c" else b"\xc3")
        read_for(0.2)
        if stop == "ctrl-c":
            os.write(master, b"\x03")
        else:
            process.send_signal(signal.SIGTERM)
        deadline = time.monotonic() + 5
        while process.poll() is None and time.monotonic() < deadline:
            read_for(0.05)
        assert process.poll() == 0, "clean shutdown failed"
        read_for(0.05)
        check_image_payloads(output)
        assert b"\x1b[?1049l" in output, "alternate screen not restored"
        # macOS revokes the controlling slave when its session leader exits.
        assert termios.tcgetattr(master) == initial, "input settings not restored"
        assert fcntl.fcntl(slave, fcntl.F_GETFL) & os.O_NONBLOCK == original_flags & os.O_NONBLOCK, "input blocking mode not restored"
        if stop == "sigterm":
            assert not select.select([slave], [], [], 0)[0], "input leaked back to shell"
        print(f"PASS: echo={echo}, {stop}, no echo/overflow bell, partial input shutdown, restored settings and empty input")
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    assert len(sys.argv) > 1, __doc__
    for echo in (termios.ECHO | termios.ECHONL, 0):
        for stop in ("ctrl-c", "sigterm"):
            check(echo, stop)
