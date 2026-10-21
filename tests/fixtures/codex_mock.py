"""Deterministic Codex app-server peer; no network or actual credentials."""

import json
import os
import sys
from pathlib import Path


def write(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)


connected = Path(os.environ["CODEX_HOME"]) / "test-connected"
selected_model = None
pim_enabled = False
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
    elif method == "model/list":
        assert params["limit"] == 100 and not params["includeHidden"]
        if params["cursor"] is None:
            result = {
                "data": [{"id": "display-id", "model": "test-model"}],
                "nextCursor": "second-page",
            }
        else:
            assert params["cursor"] == "second-page"
            result = {
                "data": [{"id": "other-display-id", "model": "gpt-6-luna"}],
                "nextCursor": None,
            }
    elif method == "thread/start":
        assert params["model"] in ("test-model", "gpt-6-luna")
        selected_model = params["model"]
        assert params["ephemeral"] and params["sandbox"] == "read-only"
        assert not params["config"]["features.shell_tool"]
        assert any(t["name"] == "read_mail" for t in params["dynamicTools"])
        pim_enabled = any(t["name"] == "list_calendars" for t in params["dynamicTools"])
        if pim_enabled:
            assert any(
                t["name"] == "propose_create_contact" for t in params["dynamicTools"]
            )
        result = {"thread": {"id": "thread-test"}}
    elif method == "turn/start":
        assert params["model"] == selected_model
        if selected_model == "gpt-6-luna":
            write({"id": request["id"], "result": {"turn": {"id": "turn-test"}}})
            error = {
                "type": "error",
                "status": 400,
                "error": {
                    "type": "invalid_request_error",
                    "message": "The 'gpt-6-luna' model is not supported when using Codex with a ChatGPT account.",
                },
            }
            write(
                {"method": "error", "params": {"error": {"message": json.dumps(error)}}}
            )
            continue
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
        if pim_enabled:
            write(
                {
                    "id": "bad-args",
                    "method": "item/tool/call",
                    "params": {"tool": "search_mail", "arguments": "{invalid JSON"},
                }
            )
            failed = json.loads(sys.stdin.readline())
            assert failed["id"] == "bad-args" and not failed["result"]["success"]
            assert "Invalid JSON" in failed["result"]["contentItems"][0]["text"]
            calls = [
                ("list_calendars", {}),
                ("list_address_books", {}),
                (
                    "propose_create_event",
                    {
                        "calendar": "cal",
                        "summary": "LLM meeting",
                        "start": "20261201T090000Z",
                        "end": "20261201T100000Z",
                    },
                ),
                (
                    "propose_create_contact",
                    {
                        "book": "book",
                        "name": "LLM contact",
                        "email": "person@example.test",
                    },
                ),
            ]
            for number, (name, arguments) in enumerate(calls):
                tool_id = f"pim-{number}"
                write(
                    {
                        "id": tool_id,
                        "method": "item/tool/call",
                        "params": {"tool": name, "arguments": arguments},
                    }
                )
                result = json.loads(sys.stdin.readline())
                assert result["id"] == tool_id and result["result"]["success"], result
                data = json.loads(result["result"]["contentItems"][0]["text"])
                if name.startswith("propose_"):
                    assert data["status"] == "proposed" and not data["applied"]
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
