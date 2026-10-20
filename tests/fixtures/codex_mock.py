"""Deterministic Codex app-server peer; no network or actual credentials."""

import json
import os
import sys
from pathlib import Path


def write(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)


connected = Path(os.environ["CODEX_HOME"]) / "test-connected"
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    params = request.get("params", {})
    result = {}
    if method == "initialize":
        assert params["capabilities"]["experimentalApi"]
    elif method == "initialized":
        continue
    elif method == "account/login/start":
        assert params["type"] == "chatgpt"
        connected.touch()
        write(
            {
                "id": request["id"],
                "result": {
                    "authUrl": "https://example.invalid/oauth",
                    "loginId": "test-login",
                },
            }
        )
        write(
            {
                "method": "account/login/completed",
                "params": {"loginId": "test-login", "success": True},
            }
        )
        continue
    elif method == "account/read":
        result = {"account": {"type": "chatgpt"} if connected.exists() else None}
    elif method == "account/logout":
        connected.unlink(missing_ok=True)
    elif method == "thread/start":
        assert params["model"] == "test-model"
        assert params["ephemeral"] and params["sandbox"] == "read-only"
        assert not params["config"]["features.shell_tool"]
        assert any(t["name"] == "read_mail" for t in params["dynamicTools"])
        result = {"thread": {"id": "thread-test"}}
    elif method == "turn/start":
        assert "Incident" in params["input"][0]["text"]
        # Server requests can arrive before the turn/start acknowledgement.
        write(
            {
                "id": "tool-1",
                "method": "item/tool/call",
                "params": {"tool": "read_mail", "arguments": {"source": 1}},
            }
        )
        write({"id": request["id"], "result": {"turn": {"id": "turn-test"}}})
        tool = json.loads(sys.stdin.readline())
        assert tool["id"] == "tool-1" and tool["result"]["success"]
        value = json.loads(tool["result"]["contentItems"][0]["text"])
        assert value["source"]["message"]["id"]["account"] == "work"
        write(
            {
                "method": "item/agentMessage/delta",
                "params": {"itemId": "answer", "delta": "Contexte historique [1]"},
            }
        )
        write(
            {
                "method": "item/completed",
                "params": {
                    "item": {
                        "id": "answer",
                        "type": "agentMessage",
                        "text": "Contexte historique [1]",
                    }
                },
            }
        )
        write({"method": "turn/completed", "params": {"turn": {"status": "completed"}}})
        continue
    else:
        raise AssertionError(request)
    write({"id": request["id"], "result": result})
