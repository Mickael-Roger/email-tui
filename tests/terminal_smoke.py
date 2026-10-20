#!/usr/bin/env python3
"""Linux pseudo-terminal acceptance test; never opens a real mailbox."""

import errno
import fcntl
import http.server
import json
import os
import pty
import re
import select
import signal
import struct
import sys
import tempfile
import termios
import time
import threading


def main():
    binary = os.path.abspath(
        sys.argv[1] if len(sys.argv) > 1 else "target/debug/mailtui"
    )
    with tempfile.TemporaryDirectory(prefix="mailtui-pty-") as directory:
        calls = []

        class Llm(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                assert self.path == "/v1/chat/completions"
                assert self.headers["Authorization"] == "Bearer terminal-fixture-key"
                request = json.loads(
                    self.rfile.read(int(self.headers["Content-Length"]))
                )
                assert request["model"] == "terminal-fixture-model"
                calls.append(request)
                data = {
                    "choices": [
                        {
                            "delta": {"content": "PTY assistant reply [1]"},
                            "finish_reason": "stop",
                        }
                    ]
                }
                wire = ("data: " + json.dumps(data) + "\n\ndata: [DONE]\n\n").encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(wire)))
                self.end_headers()
                self.wfile.write(wire)

            def log_message(self, format, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Llm)
        config = os.path.join(directory, "config.toml")
        with open(config, "w", encoding="utf-8") as handle:
            handle.write(
                '[viewer]\nmode = "manual"\n'
                '[llm]\nenabled = true\nmodel = "terminal-fixture-model"\n'
                f'endpoint = "http://127.0.0.1:{server.server_port}/v1"\n'
                '[llm.auth]\napi_key_env = "MAILTUI_PTY_LLM_KEY"\n'
            )
        pid, master = pty.fork()
        if pid == 0:
            os.environ["TERM"] = "xterm-256color"
            os.environ["XDG_DOWNLOAD_DIR"] = os.path.join(directory, "downloads")
            os.environ["MAILTUI_PTY_LLM_KEY"] = "terminal-fixture-key"
            os.execv(binary, [binary, "--demo", "--config", config])
        threading.Thread(target=server.serve_forever, daemon=True).start()
        original = termios.tcgetattr(master)
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))

        def expect(needle, timeout=8):
            data = b""
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.1)[0]:
                    try:
                        data += os.read(master, 65536)
                    except OSError as error:
                        if error.errno == errno.EIO:
                            break
                        raise
                    if needle in data:
                        return
            readable = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", data)
            readable = re.sub(rb"\s+", b" ", readable)
            raise AssertionError(
                f"Terminal did not show {needle!r}; output: {readable!r}"
            )

        def drain():
            # Keep stdout draining until rendering is idle, so a previous
            # frame cannot block the application on the bounded PTY buffer.
            while select.select([master], [], [], 0.2)[0]:
                os.read(master, 65536)

        width = 120

        def send(keys, needle):
            nonlocal width
            drain()
            os.write(master, keys)
            drain()
            # A full redraw makes assertions independent of cell diff encoding.
            width = 121 if width == 120 else 120
            fcntl.ioctl(
                master, termios.TIOCSWINSZ, struct.pack("HHHH", 32, width, 0, 0)
            )
            os.kill(pid, signal.SIGWINCH)
            expect(needle)

        try:
            expect(b"DEMO")
            send(b"1", b"Messages(work)")
            send(b":inbox\r", b"Messages(work)[55 mail")
            send(b":dir Projects/Rust/MailTUI\r", b"Messages(work)[64 mail")
            send(
                b"\r", b"Messages(work)[55 mail"
            )  # Fold only that folder's nine mails.
            send(
                b"\r", b"Messages(work)[64 mail"
            )  # Unfold cached mail in the same list.
            send(b":inbox\r", b"Messages(work)[64 mail")
            send(b"2", b"Messages(personal)")
            send(b"0", b"Messages(all)")
            send(b":ns\r", b"ADDRESS")
            send(b":directories\r", b"NAME / SUBJECT")
            send(b":unified\r", b"SUBJECT")
            send(b"\x1b[B ", b"1 marked")  # Inbox heading -> first message.
            send(b"\x04", b"Delete messages?")
            send(b"\r", b"Messages(all)[119 mail")  # Cancel is the default button.
            send(b"\x04\t\r", b"Messages(all)[118 mail")
            send(b":reload\r", b"Messages(all)[118 mail")
            send(b"n", b"Create directory")
            send(b"Travel\r", b"15 folders")
            send(b":inbox\r", b"Messages(all)[118 mail")
            send(b"\x1b[Bm", b"fuzzy directory filter")
            send(b"arv", b"Archive")
            send(b"\x1b", b"Messages(all)")
            send(b"c", b"Compose email")
            send(b"\x14", b"Attach files")
            send(b"\x1b", b"Compose email")
            send(b"alice@example.com\tPTY draft\tTwo lines\rBody\x0f", b"Messages(all)")
            send(b"/", b"MailTUI local index")
            send(b"from:alice@example.com kubernetes", b"Release planning")
            send(b"\r", b"Hello team")
            send(b"\x13", b"select files to save")
            send(b"\r", b"Folder:")
            send(b"\r", b"Saved 1 attachment")
            with open(os.path.join(directory, "downloads", "logo.png"), "rb") as saved:
                assert saved.read().startswith(b"\x89PNG"), (
                    "Attachment was not decoded and saved"
                )
            assert not calls, "Assistant contacted the endpoint before a question"
            send(b"\x01", b"Search scope:")
            send(b"Summarize this message\r", b"PTY assistant reply [1]")
            send(b"\x1b", b"Hello team")
            send(b":ai\r", b"PTY assistant reply [1]")
            send(b"/sources\r", b"Release planning")
            send(b"/open 1\r", b"Hello team")
            assert len(calls) == 1, (
                "Reopening the conversation repeated the LLM request"
            )
            send(b"h", b"private URL")
            send(b"\x1b", b"Hello team")
            send(b"H", b"/remote")
            os.write(master, b"\x1b")
            time.sleep(0.1)
            # Resize in detail and back: event-driven redraw must remain usable.
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 12, 40, 0, 0))
            os.kill(pid, signal.SIGWINCH)
            time.sleep(0.1)
            send(b"\x03", b"Ctrl+C again")
            os.write(master, b"\x03")
            deadline = time.monotonic() + 8
            status = None
            while time.monotonic() < deadline:
                waited, code = os.waitpid(pid, os.WNOHANG)
                if waited:
                    status = code
                    break
                if select.select([master], [], [], 0.05)[0]:
                    try:
                        os.read(master, 65536)
                    except OSError:
                        pass
            assert status is not None, "TUI failed to exit after Ctrl+C"
            assert os.waitstatus_to_exitcode(status) == 0, f"TUI failed: {status}"
            assert termios.tcgetattr(master) == original, (
                "Terminal mode was not restored"
            )
            print(
                "PTY smoke passed: folded tree, namespaces, nested folders, Ctrl+D, m fuzzy picker, directory creation, c composer/draft, local reload, MIME, attachment download, Ctrl+A/:ai conversation, streaming, sources, HTML URL, resize, terminal restoration"
            )
        finally:
            try:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            except (ProcessLookupError, ChildProcessError):
                pass
            os.close(master)
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
