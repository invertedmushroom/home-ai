"""Opt-in synthetic chat/app/file checks against a running Deep server. No API keys needed here."""

import argparse
import json
import time
import urllib.error
import urllib.parse
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Loopback Deep URL; use an isolated data directory")
    parser.add_argument("--backend", action="append", required=True, help="Configured backend ID (repeatable)")
    parser.add_argument("--mode", action="append", choices=("chat", "app", "files"), help="Defaults to chat and app; app/files inspect pending proposals without applying")
    parser.add_argument("--require-draft", action="store_true", help="Require observing partial text before completion")
    args = parser.parse_args()
    parsed = urllib.parse.urlsplit(args.url)
    if (parsed.scheme != "http" or parsed.hostname not in ("localhost", "127.0.0.1", "::1")
            or parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path not in ("", "/")):
        parser.error("--url must be a plain loopback HTTP URL")
    base = args.url.rstrip("/")

    def request(path, body=None, method=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(base + "/api" + path, data=data, method=method,
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=15) as response:
            return json.load(response)

    status = request("/status")
    configured = {backend["id"]: backend["model"] for backend in status["backends"]}
    if any(backend not in configured for backend in args.backend):
        parser.error("all --backend values must be configured in the running server")
    failed = False
    for backend in args.backend:
        for mode in args.mode or ("chat", "app"):
            workspace = request("/workspaces", {"title": f"Smoke {backend} {mode}"})
            prompt = ("What is 19 + 23? Answer with only the number." if mode == "chat" else
                      "Build a tiny counter starting at 0 with an Add one button. "
                      "Use one self-contained HTML document, inline JavaScript and no external assets.")
            if mode == "files":
                prompt = ("Propose a tiny counter with visible #count starting at 0 and an Add one button. "
                          "Use exactly index.html, style.css and app.js with relative classic script/style links. "
                          "Keep the files minimal; no modules, packages or external assets.")
            start = time.monotonic()
            submission = {"prompt": prompt, "mode": mode, "backend": backend}
            if mode == "files":
                submission["base_revision"] = {"id": None}
            run = request(f"/workspaces/{workspace['id']}/runs", submission)
            deadline = start + 240
            saw_draft = False
            try:
                while run["status"] in ("queued", "running"):
                    if time.monotonic() >= deadline:
                        raise TimeoutError("smoke deadline")
                    time.sleep(0.25)
                    run = request(f"/runs/{run['id']}")
                    saw_draft |= run["status"] == "running" and bool(run.get("output"))
                detail = request(f"/workspaces/{workspace['id']}")
                correct = run["provider"] == backend and run["model"] == configured[backend]
                if mode == "chat":
                    correct &= bool(detail["messages"]) and detail["messages"][-1]["content"].strip() == "42"
                else:
                    proposals = detail["proposals"]
                    correct &= len(detail["revisions"]) == 0 and len(proposals) == 1
                    if proposals:
                        correct &= (proposals[0]["run_id"] == run["id"] and proposals[0]["status"] == "pending"
                                    and {change["path"] for change in proposals[0]["changes"]} ==
                                    ({"index.html"} if mode == "app" else {"index.html", "style.css", "app.js"}))
                passed = run["status"] == "succeeded" and correct
                if args.require_draft:
                    passed &= saw_draft
                failed |= not passed
                print(json.dumps({"backend": backend, "configured_model": configured[backend], "mode": mode,
                                  "status": run["status"], "passed": passed,
                                  "elapsed_seconds": round(time.monotonic() - start, 2), "saw_draft": saw_draft,
                                  "queue_ms": run.get("queue_ms"), "duration_ms": run.get("duration_ms"),
                                  "stats": run.get("stats"), "error": run["error"]}), flush=True)
            finally:
                if run["status"] in ("queued", "running"):
                    request(f"/runs/{run['id']}", method="DELETE")
    return int(failed)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (urllib.error.URLError, TimeoutError) as error:
        print(json.dumps({"error": type(error).__name__}))
        raise SystemExit(1)
