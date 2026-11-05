"""The script is a minimal stdio MCP server for the agent tests."""
import json
import sys

SCHEMA = {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
        "text": {"type": "string", "format": "plain", "default": "hi"},
        "mode": {"anyOf": [{"type": "string"}, {"type": "null"}]},
    },
    "required": ["text", "mode"],
    "additionalProperties": False,
}

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if "id" not in message:
        continue
    if method == "initialize":
        result = {
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "echo", "version": "1.0.0"},
        }
    elif method == "tools/list":
        result = {"tools": [{"name": "say", "description": "Echo text.", "inputSchema": SCHEMA}]}
    elif method == "tools/call":
        text = message["params"]["arguments"]["text"]
        result = {"content": [{"type": "text", "text": "echo: " + text}], "isError": False}
    else:
        reply = {"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": method}}
        print(json.dumps(reply), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
