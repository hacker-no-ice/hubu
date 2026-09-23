#!/usr/bin/env python3
"""Reference external executor for the Hubu executor conformance runner.

This is the smallest possible executor-side plugin. It speaks the
``hubu-executor-conformance-plugin-v1`` JSON-lines protocol on stdin/stdout and
performs each executor operation against Hubu with its own normal bearer from
``HUBU_CONFORMANCE_EXECUTOR_TOKEN``. A real executor replaces ``perform`` with
calls through its own Hubu client while keeping the same observed responses.

It never receives or uses the human reconciliation capability.
"""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

PROTOCOL = "hubu-executor-conformance-plugin-v1"
RECONCILIATION_HEADER = "X-Hubu-Reconciliation-Capability"


def perform(base_url: str, bearer: str, message: dict) -> dict:
    url = base_url.rstrip("/") + message["path"]
    if message.get("query"):
        url += "?" + urllib.parse.urlencode(message["query"])
    headers = {"Authorization": f"Bearer {bearer}", "Accept": "application/json"}
    data = None
    if message.get("body") is not None:
        data = json.dumps(message["body"]).encode()
        headers["Content-Type"] = "application/json"
    if message.get("reconciliation_capability") == "executor_bearer":
        # Conformance probe: an executor presenting its own bearer as the
        # human capability must be rejected by Hubu.
        headers[RECONCILIATION_HEADER] = bearer
    request = urllib.request.Request(url, method=message["method"], data=data, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return {"status": response.status, "body": json.loads(response.read() or b"null")}
    except urllib.error.HTTPError as error:
        return {"status": error.code, "body": json.loads(error.read() or b"null")}
    except (urllib.error.URLError, OSError) as error:
        return {"transport_error": str(error)}


def main() -> int:
    bearer = os.environ.get("HUBU_CONFORMANCE_EXECUTOR_TOKEN", "hubu_conformance_normal_bearer")
    base_url = None
    for line in sys.stdin:
        message = json.loads(line)
        if message["type"] == "hello":
            if message.get("protocol") != PROTOCOL:
                reply = {"type": "error", "error": f"unsupported protocol {message.get('protocol')}"}
            else:
                base_url = message["base_url"]
                reply = {"type": "ready", "executor": "reference-python"}
        elif message["type"] == "invoke":
            reply = perform(base_url, bearer, message)
        elif message["type"] == "shutdown":
            print(json.dumps({"type": "bye"}), flush=True)
            return 0
        else:
            reply = {"type": "error", "error": f"unknown message type {message['type']}"}
        print(json.dumps(reply), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
