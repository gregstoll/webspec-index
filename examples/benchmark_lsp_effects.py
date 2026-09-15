#!/usr/bin/env python3
"""Replay editor requests against a source file; bound time and Linux RSS.

Usage: python3 examples/benchmark_lsp_effects.py FILE --anchor ANCHOR
Uses the installed server and its normal cache unless environment overrides it.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("file", type=Path)
    parser.add_argument("--anchor", required=True)
    parser.add_argument("--server", default="webspec-index")
    parser.add_argument("--seconds", type=float, default=30)
    parser.add_argument("--max-rss-mib", type=int, default=1024)
    parser.add_argument("--responses", type=Path, help="Save protocol responses for UI inspection")
    parser.add_argument("--click-effects", action="store_true", help="Replay clicks on prepared effect categories for the requested algorithm")
    args = parser.parse_args()
    text = args.file.read_text()
    lines = text.splitlines()
    line = next(i for i, value in enumerate(lines) if "#" + args.anchor in value)
    character = lines[line].index("https://") + 10
    uri = args.file.resolve().as_uri()
    inbox = queue.Queue()
    stderr = Path("/tmp/webspec-lsp-benchmark-stderr.log").open("w")
    process = subprocess.Popen([args.server, "lsp"], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=stderr)

    def read_messages():
        while True:
            length = None
            while True:
                header = process.stdout.readline()
                if not header:
                    return
                if header == b"\r\n":
                    break
                if header.lower().startswith(b"content-length:"):
                    length = int(header.split(b":", 1)[1])
            inbox.put(json.loads(process.stdout.read(length)))

    threading.Thread(target=read_messages, daemon=True).start()
    pending = set()
    request_times = {}
    request_methods = {}
    click_ms = []
    clicked_categories = set()
    effect_lenses = 0
    next_id = 0

    def send(method=None, params=None, request=False, **extra):
        nonlocal next_id
        message = {"jsonrpc": "2.0", **extra}
        if method:
            message.update(method=method, params=params)
        if request:
            next_id += 1
            message["id"] = next_id
            pending.add(next_id)
            request_times[next_id] = time.monotonic()
            request_methods[next_id] = method
        body = json.dumps(message).encode()
        process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        process.stdin.flush()
        return next_id

    hint_params = {"textDocument": {"uri": uri}, "range": {
        "start": {"line": 0, "character": 0},
        "end": {"line": len(lines), "character": 0}}}
    start = time.monotonic()
    peak_rss = peak_threads = refreshes = responses = 0
    max_hints = effect_hints = 0
    errors = []
    captured = []
    cpu_seconds = 0
    last_busy = start
    previous_cpu = 0
    initialized = False
    settled = False
    try:
        send("initialize", {"processId": None, "rootUri": None,
             "capabilities": {"workspace": {"inlayHint": {"refreshSupport": True}, "codeLens": {"refreshSupport": True}}},
             "initializationOptions": {"effectsEnabled": True}}, request=True)
        while time.monotonic() - start < args.seconds:
            if process.poll() is not None:
                raise RuntimeError(f"LSP exited: {process.returncode}")
            status = Path(f"/proc/{process.pid}/status").read_text().splitlines()
            rss = int(next(s.split()[1] for s in status if s.startswith("VmRSS:")))
            threads = int(next(s.split()[1] for s in status if s.startswith("Threads:")))
            peak_rss, peak_threads = max(peak_rss, rss), max(peak_threads, threads)
            if rss > args.max_rss_mib * 1024:
                raise RuntimeError("RSS watchdog exceeded")
            stat = Path(f"/proc/{process.pid}/stat").read_text().rsplit(")", 1)[1].split()
            cpu_seconds = (int(stat[11]) + int(stat[12])) / os.sysconf("SC_CLK_TCK")
            if cpu_seconds != previous_cpu:
                last_busy = time.monotonic()
            previous_cpu = cpu_seconds
            try:
                message = inbox.get(timeout=0.05)
            except queue.Empty:
                if initialized and not pending and time.monotonic() - last_busy > 3:
                    settled = True
                    break
                continue
            last_busy = time.monotonic()
            if "method" in message:
                if "id" in message:
                    send(id=message["id"], result=None)
                if message["method"] == "workspace/inlayHint/refresh":
                    refreshes += 1
                    send("textDocument/inlayHint", hint_params, request=True)
                if message["method"] == "workspace/codeLens/refresh":
                    send("textDocument/codeLens", {"textDocument": {"uri": uri}}, request=True)
                continue
            if request_methods.get(message.get("id")) == "webspec/preparedEffectDocument":
                click_ms.append(round((time.monotonic() - request_times[message["id"]]) * 1000, 2))
            pending.discard(message.get("id"))
            responses += 1
            if "error" in message:
                errors.append(message["error"])
            if args.responses:
                captured.append(message)
            result = message.get("result")
            if isinstance(result, list) and result and "label" in result[0]:
                max_hints = max(max_hints, len(result))
                effect_hints = max(effect_hints, sum("[may " in h["label"] for h in result))
            if isinstance(result, list) and result and "command" in result[0]:
                clickable = [lens for lens in result if lens.get("command", {}).get("command") == "webspecLens.showEffects"]
                effect_lenses = max(effect_lenses, len(clickable))
                if args.click_effects:
                    for lens in clickable:
                        request = lens["command"]["arguments"][0]
                        category = request.get("category")
                        if request["subject"]["anchor"] == args.anchor and category not in clicked_categories:
                            clicked_categories.add(category)
                            send("webspec/preparedEffectDocument", request, request=True)
            if message.get("id") == 1:
                initialized = True
                send("initialized", {})
                send("textDocument/didOpen", {"textDocument": {
                    "uri": uri, "languageId": "cpp", "version": 1, "text": text}})
                send("textDocument/hover", {"textDocument": {"uri": uri},
                     "position": {"line": line, "character": character}}, request=True)
                send("textDocument/inlayHint", hint_params, request=True)
                send("textDocument/codeLens", {"textDocument": {"uri": uri}}, request=True)
    finally:
        process.kill()
        process.wait()
        stderr.close()
        if args.responses:
            args.responses.write_text(json.dumps(captured, indent=2))
        print(json.dumps({"file": str(args.file), "anchor": args.anchor,
              "seconds": round(time.monotonic() - start, 2), "cpu_seconds": cpu_seconds,
              "peak_rss_mib": round(peak_rss / 1024, 1), "peak_threads": peak_threads,
              "refreshes": refreshes, "responses": responses, "max_hints": max_hints,
              "effect_hints": effect_hints, "effect_lenses": effect_lenses, "click_ms": click_ms, "pending": len(pending), "settled": settled,
              "errors": errors}, indent=2))


if __name__ == "__main__":
    main()
