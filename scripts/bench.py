#!/usr/bin/env python3
"""Offline performance benchmark for the webspec-index CLI.

Runs every workload as a fresh process against a private copy of the index,
so the real database (~/.webspec-index/index.db) is only ever read. Writes a
JSON result and a markdown table.

    cargo build --release --bin webspec-index
    python3 scripts/bench.py                    # everything
    python3 scripts/bench.py --skip-indexing    # runtime lookups only
    python3 scripts/bench.py --only query       # workloads whose name contains "query"
    python3 scripts/bench.py --full             # adds a one-shot reparse of every cached spec
    python3 scripts/bench.py --network          # adds real conditional-GET batch (network access)

Isolation:
  * The source database is copied with SQLite's online backup API (read-only
    on the source) to target/bench/db/index.db. The binary is pointed at the
    copy with SPEC_INDEX_TEST_DB; target/bench/db/html is a symlink to the
    source's HTML cache, which `reparse` reads and never writes.
  * `update_checks.last_checked` is set to now in the copy so lookups stay
    inside the 24h freshness window instead of re-fetching.
  * MOZTOOLS_UPDATE_CHECK=0 disables the crates.io version check.
  * Every process runs under `unshare -rn` (no network namespace) when the
    kernel allows it; a workload that tries the network fails instead of
    silently timing a download.
  * query-change workloads use a private html/ directory with a deterministically
    edited copy of the HTML spec, served by a local HTTP stub that honours
    If-None-Match. WEBSPEC_FETCH_ORIGIN points child processes at the stub.
    The network guard is omitted for these workloads so they can reach localhost.

Only the Python standard library is used. Per-run wall time is measured
around spawn + wait; user/sys CPU come from wait4(2), peak RSS from GNU time
(/usr/bin/time) when installed, else from wait4(2) with Python's own RSS as a
floor.
"""

from __future__ import annotations

import argparse
import datetime as dt
import http.server
import json
import math
import os
import platform
import shutil
import socket
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

_STUB_ETAG = '"bench-1"'
_STUB_HTML_PATH = "/html.spec.whatwg.org/"


class _StubHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == _STUB_HTML_PATH:
            if self.headers.get("If-None-Match", "") == _STUB_ETAG:
                self.send_response(304)
                self.send_header("ETag", _STUB_ETAG)
                self.end_headers()
            else:
                body: bytes = self.server.html_body  # type: ignore[attr-defined]
                self.send_response(200)
                self.send_header("ETag", _STUB_ETAG)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, fmt: str, *args: object) -> None:
        pass


class StubServer:
    """Local HTTP server that serves a deterministically edited HTML spec.

    The served content is the cached HTML file with ``<!-- bench-edit -->``
    appended, so it has a different content hash than what is stored in the
    bench DB. ``WEBSPEC_FETCH_ORIGIN`` set to :attr:`origin` routes the binary's
    fetch requests here instead of to the real spec host.
    """

    def __init__(self, html_dir: Path) -> None:
        html_spec_dir = html_dir / "HTML"
        html_files = list(html_spec_dir.glob("*.html"))
        if not html_files:
            raise FileNotFoundError(f"no cached HTML spec at {html_spec_dir}")
        original = html_files[0].read_bytes()
        body = original + b"\n<!-- bench-edit -->\n"

        srv = http.server.HTTPServer(("127.0.0.1", 0), _StubHandler)
        srv.html_body = body  # type: ignore[attr-defined]
        self._server = srv
        self._port: int = srv.server_address[1]
        self._thread = threading.Thread(target=srv.serve_forever, daemon=True)
        self._thread.start()

    @property
    def origin(self) -> str:
        return f"http://127.0.0.1:{self._port}"

    def stop(self) -> None:
        self._server.shutdown()


@dataclass
class Workload:
    name: str
    args: list[str]
    group: str  # "startup", "runtime", "indexing", "indexing-full", "query-change", "network"
    runs: int | None = None
    warmup: int | None = None
    note: str = ""
    fresh_db: bool = False    # copy source DB fresh before each run (not shared with other runs)
    clear_memo: bool = False  # delete from markdown_memo before timing (implies fresh_db)
    no_guard: bool = False    # skip the network guard (needed for stub workloads)
    network_only: bool = False  # only run when --network is given


WORKLOADS = [
    Workload("version", ["--version"], "startup",
             note="argument check only; no DB open"),
    Workload("specs", ["specs"], "startup",
             note="DB open + schema/migrations/purge check + spec seed + one small query"),
    Workload("exists", ["exists", "HTML#navigate"], "runtime"),
    Workload("query-navigate-json", ["query", "HTML#navigate"], "runtime",
             note="default --effects auto (stored preview)"),
    Workload("query-navigate-md", ["query", "HTML#navigate", "--format", "markdown"], "runtime"),
    Workload("query-navigate-effects-cached", ["query", "HTML#navigate", "--effects", "cached"], "runtime"),
    Workload("query-navigate-effects-off", ["query", "HTML#navigate", "--effects", "off"], "runtime"),
    Workload("query-navigate-involving", ["query", "HTML#navigate", "--involving", "historyHandling"], "runtime"),
    Workload("query-navigate-feeding", ["query", "HTML#navigate", "--feeding", "24.9.1"], "runtime"),
    Workload("query-navigate-depth1", ["query", "HTML#navigate", "--depth", "1"], "runtime"),
    Workload("query-dom-insert-involving", ["query", "DOM#concept-node-insert", "--involving", "parent"], "runtime"),
    Workload("query-dom-insert-effects-off", ["query", "DOM#concept-node-insert", "--effects", "off"], "runtime"),
    Workload("query-dom-small", ["query", "DOM#concept-tree-root"], "runtime"),
    Workload("query-dom-small-effects-off", ["query", "DOM#concept-tree-root", "--effects", "off"], "runtime"),
    Workload("refs-incoming-200", ["refs", "HTML#navigate", "-d", "incoming", "-l", "200"], "runtime"),
    Workload("refs-both-default", ["refs", "HTML#navigate"], "runtime"),
    Workload("search-dom", ["search", "tree order", "-s", "DOM"], "runtime"),
    Workload("search-html", ["search", "tree order", "-s", "HTML"], "runtime"),
    Workload("search-all", ["search", "tree order"], "runtime", note="FTS across all specs"),
    Workload("anchors-dom", ["anchors", "*-tree", "-s", "DOM"], "runtime"),
    Workload("anchors-all", ["anchors", "concept-*"], "runtime", note="glob across all specs"),
    Workload("idl-window-open", ["idl", "Window.open()"], "runtime"),
    Workload("list-html", ["list", "HTML"], "runtime"),
    Workload("trace-assign-navigateerror",
             ["trace", "HTML#dom-location-assign", "HTML#event-navigateerror"], "runtime"),
    Workload("graph-navigate", ["graph", "HTML#navigate"], "runtime"),
    Workload("flow-navigate", ["flow", "HTML#navigate"], "runtime"),
    Workload("effects-navigate-summary", ["effects", "HTML#navigate", "--summary-only"], "runtime",
             runs=8, warmup=1),
    Workload("effects-navigate", ["effects", "HTML#navigate"], "runtime", runs=8, warmup=1),
    Workload("reparse-dom", ["reparse", "-s", "DOM", "--effects", "off"], "indexing"),
    Workload("reparse-html", ["reparse", "-s", "HTML", "--effects", "off"], "indexing"),
    Workload("effects-all", ["effects", "--all"], "indexing",
             note="incremental effects build run by `update`/`reparse`"),
    Workload("effects-all-full", ["effects", "--all", "--rebuild"], "indexing",
             note="full effects rebuild from scratch (≤5 s / 1 GB gate)"),
    Workload("reparse-html-memo-hit", ["reparse", "-s", "HTML", "--effects", "off"], "indexing",
             runs=3, warmup=1, fresh_db=True,
             note="re-parse HTML from cache with markdown memo hits (≤2.5 s gate)"),
    Workload("reparse-html-cold", ["reparse", "-s", "HTML", "--effects", "off"], "indexing",
             runs=3, warmup=1, fresh_db=True, clear_memo=True,
             note="re-parse HTML from cache with cold memo"),
    Workload("reparse-all", ["reparse", "--effects", "off"], "indexing-full", runs=1, warmup=0,
             note="every cached spec in parallel; only with --full"),
    Workload("query-after-html-change", ["query", "HTML#navigate"], "query-change",
             runs=3, warmup=1, no_guard=True,
             note="first query after HTML changed; triggers freshness check, re-parse and "
                  "inline effects rebuild (≤4.5 s gate excluding network)"),
    Workload("query-after-html-change-2nd", ["query", "HTML#navigate"], "query-change",
             runs=3, warmup=0, no_guard=True,
             note="second query after HTML changed; served from the new publication (≤20 ms gate)"),
    Workload("effects-incremental-html", ["effects", "--all"], "query-change",
             runs=3, warmup=1, no_guard=True,
             note="incremental effects after HTML spec changed (≤2.5 s / 500 MB gate)"),
    Workload("update-all-network", ["update", "--effects", "off"], "network",
             runs=1, warmup=0, no_guard=True, fresh_db=True, network_only=True,
             note="real conditional-GET freshness batch for all indexed specs; "
                  "stale last_checked triggers one check per spec; opt-in with --network"),
]


@dataclass
class Sample:
    wall_ms: float
    user_ms: float
    sys_ms: float
    maxrss_kb: int
    exit_code: int
    stdout_bytes: int


@dataclass
class Result:
    workload: Workload
    samples: list[Sample] = field(default_factory=list)

    def stats(self) -> dict:
        walls = sorted(s.wall_ms for s in self.samples)
        return {
            "name": self.workload.name,
            "group": self.workload.group,
            "args": self.workload.args,
            "note": self.workload.note,
            "runs": len(walls),
            "wall_ms": {
                "min": walls[0],
                "median": statistics.median(walls),
                "mean": statistics.fmean(walls),
                "p95": percentile(walls, 95),
                "max": walls[-1],
                "stdev": statistics.stdev(walls) if len(walls) > 1 else 0.0,
            },
            "user_ms_median": statistics.median(s.user_ms for s in self.samples),
            "sys_ms_median": statistics.median(s.sys_ms for s in self.samples),
            "maxrss_kb_max": max(s.maxrss_kb for s in self.samples),
            "exit_codes": sorted({s.exit_code for s in self.samples}),
            "stdout_bytes": self.samples[-1].stdout_bytes,
            "samples_wall_ms": [s.wall_ms for s in self.samples],
        }


def percentile(sorted_values: list[float], pct: float) -> float:
    rank = max(1, math.ceil(pct / 100 * len(sorted_values)))
    return sorted_values[rank - 1]


GNU_TIME = Path("/usr/bin/time")


def run_once(cmd: list[str], env: dict, tmp: Path) -> Sample:
    stdout_path = tmp / "stdout"
    rss_path = tmp / "rss"
    # A child's peak RSS starts at its forking parent's, so wait4 on a child of
    # this Python process reports at least Python's own RSS. GNU time forks the
    # benchmarked process from a small image and reports its own peak.
    if GNU_TIME.exists():
        cmd = [str(GNU_TIME), "-f", "%M", "-o", str(rss_path)] + cmd
    with open(stdout_path, "wb") as out:
        start = time.perf_counter()
        proc = subprocess.Popen(cmd, env=env, stdout=out, stderr=subprocess.DEVNULL)
        _, status, usage = os.wait4(proc.pid, 0)
        wall = (time.perf_counter() - start) * 1000
    proc.returncode = os.waitstatus_to_exitcode(status)
    maxrss = usage.ru_maxrss
    if GNU_TIME.exists():
        maxrss = int(rss_path.read_text().split()[-1])
    return Sample(
        wall_ms=wall,
        user_ms=usage.ru_utime * 1000,
        sys_ms=usage.ru_stime * 1000,
        maxrss_kb=maxrss,
        exit_code=proc.returncode,
        stdout_bytes=stdout_path.stat().st_size,
    )


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True).stdout.strip()


def machine_info() -> dict:
    cpu = ""
    mem_kb = 0
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                mem_kb = int(line.split()[1])
    except OSError:
        pass
    governor = ""
    gov_path = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
    if gov_path.exists():
        governor = gov_path.read_text().strip()
    return {
        "cpu": cpu or platform.processor(),
        "logical_cores": os.cpu_count(),
        "mem_gib": round(mem_kb / 1024 / 1024, 1),
        "kernel": platform.release(),
        "cpu_governor": governor,
        "rustc": subprocess.run(["rustc", "--version"], capture_output=True, text=True).stdout.strip(),
    }


def db_info(path: Path) -> dict:
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        q = lambda sql: conn.execute(sql).fetchone()[0]  # noqa: E731
        return {
            "path": str(path),
            "bytes": path.stat().st_size,
            "page_size": q("PRAGMA page_size"),
            "page_count": q("PRAGMA page_count"),
            "freelist_count": q("PRAGMA freelist_count"),
            "journal_mode": q("PRAGMA journal_mode"),
            "snapshots": q("SELECT COUNT(*) FROM snapshots"),
            "sections": q("SELECT COUNT(*) FROM sections"),
            "refs": q("SELECT COUNT(*) FROM refs"),
            "index_version": q("SELECT value FROM meta WHERE key = 'index_version'"),
        }
    finally:
        conn.close()


def prepare_db(source: Path, work_dir: Path) -> Path:
    work_dir.mkdir(parents=True, exist_ok=True)
    target = work_dir / "index.db"
    for suffix in ("", "-journal", "-wal", "-shm"):
        Path(f"{target}{suffix}").unlink(missing_ok=True)
    src = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
    dst = sqlite3.connect(target)
    try:
        src.backup(dst)
        now = dt.datetime.now(dt.timezone.utc).isoformat()
        dst.execute("UPDATE update_checks SET last_checked = ?", (now,))
        dst.commit()
    finally:
        dst.close()
        src.close()
    html = work_dir / "html"
    if html.is_symlink() or html.exists():
        html.unlink()
    html.symlink_to(source.parent / "html")
    return target


def prepare_stub_db(source: Path, work_dir: Path) -> Path:
    """Like prepare_db, but sets last_checked stale for the HTML spec.

    The returned DB, combined with WEBSPEC_FETCH_ORIGIN pointing at a
    :class:`StubServer`, causes a query for an HTML anchor to trigger a
    freshness check, download the (deterministically edited) stub HTML, and
    re-parse and rebuild effects inline.
    """
    work_dir.mkdir(parents=True, exist_ok=True)
    target = work_dir / "index.db"
    for suffix in ("", "-journal", "-wal", "-shm"):
        Path(f"{target}{suffix}").unlink(missing_ok=True)
    src = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
    dst = sqlite3.connect(target)
    try:
        src.backup(dst)
        now = dt.datetime.now(dt.timezone.utc).isoformat()
        stale = "1970-01-01T00:00:00+00:00"
        dst.execute("UPDATE update_checks SET last_checked = ?", (now,))
        html_id = dst.execute(
            "SELECT id FROM specs WHERE name = 'HTML'"
        ).fetchone()
        if html_id:
            dst.execute(
                "UPDATE update_checks SET last_checked = ? WHERE spec_id = ?",
                (stale, html_id[0]),
            )
        dst.commit()
    finally:
        dst.close()
        src.close()
    html = work_dir / "html"
    if html.is_symlink() or html.exists():
        html.unlink()
    html.symlink_to(source.parent / "html")
    return target


def clear_memo_table(db_path: Path) -> None:
    conn = sqlite3.connect(db_path)
    try:
        conn.execute("DELETE FROM markdown_memo")
        conn.commit()
    finally:
        conn.close()


def network_guard() -> list[str]:
    unshare = shutil.which("unshare")
    if unshare and subprocess.run([unshare, "-rn", "true"], capture_output=True).returncode == 0:
        return [unshare, "-rn"]
    return []


def fmt_ms(ms: float) -> str:
    return f"{ms / 1000:.2f} s" if ms >= 1000 else f"{ms:.1f} ms"


def markdown(report: dict) -> str:
    m = report["meta"]
    lines = [
        f"# webspec-index benchmark {m['date']}",
        "",
        f"- Commit: `{m['git_sha']}`{' (dirty)' if m['git_dirty'] else ''}",
        f"- Machine: {m['machine']['cpu']}, {m['machine']['logical_cores']} logical cores, "
        f"{m['machine']['mem_gib']} GiB, kernel {m['machine']['kernel']}, "
        f"governor {m['machine']['cpu_governor'] or 'n/a'}",
        f"- Toolchain: {m['machine']['rustc']}; binary {m['binary_bytes'] / 1e6:.1f} MB, built {m['binary_mtime']}",
        f"- DB: {m['db']['bytes'] / 1e9:.2f} GB, {m['db']['snapshots']} snapshots, "
        f"{m['db']['sections']} sections, {m['db']['refs']} refs, index {m['db']['index_version']}",
        f"- Network guard: `{' '.join(m['network_guard']) or 'none'}`",
        "",
        "| group | workload | runs | median | p95 | min | max | user (med) | sys (med) | peak RSS | stdout |",
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for r in report["results"]:
        w = r["wall_ms"]
        flag = "" if r["exit_codes"] == [0] else f" (exit {r['exit_codes']})"
        lines.append(
            f"| {r['group']} | `{r['name']}`{flag} | {r['runs']} | {fmt_ms(w['median'])} | "
            f"{fmt_ms(w['p95'])} | {fmt_ms(w['min'])} | {fmt_ms(w['max'])} | "
            f"{fmt_ms(r['user_ms_median'])} | {fmt_ms(r['sys_ms_median'])} | "
            f"{r['maxrss_kb_max'] / 1024:.0f} MB | {r['stdout_bytes'] / 1024:.1f} KiB |"
        )
    lines.append("")
    lines.append("Commands (all prefixed with `webspec-index`):")
    lines.append("")
    for r in report["results"]:
        note = f" — {r['note']}" if r["note"] else ""
        lines.append(f"- `{r['name']}`: `{' '.join(r['args'])}`{note}")
    return "\n".join(lines) + "\n"


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--binary", type=Path, default=REPO / "target/release/webspec-index")
    p.add_argument("--commit",
                   help="commit the binary was built from, when it is not this working tree "
                        "(e.g. a clean `git archive` export); recorded instead of HEAD")
    p.add_argument("--source-db", type=Path, default=Path.home() / ".webspec-index/index.db")
    p.add_argument("--work-dir", type=Path, default=REPO / "target/bench/db")
    p.add_argument("--out-dir", type=Path, default=REPO / "target/bench/results")
    p.add_argument("--runs", type=int, default=20, help="measured runs per runtime workload")
    p.add_argument("--warmup", type=int, default=3, help="warmup runs per runtime workload")
    p.add_argument("--indexing-runs", type=int, default=3)
    p.add_argument("--indexing-warmup", type=int, default=1)
    p.add_argument("--only", action="append", default=[], help="substring filter on workload names")
    p.add_argument("--skip-indexing", action="store_true")
    p.add_argument("--skip-runtime", action="store_true")
    p.add_argument("--full", action="store_true",
                   help="also time a reparse of every cached spec (one run, minutes)")
    p.add_argument("--network", action="store_true",
                   help="include the real conditional-GET batch workload (hits the network)")
    p.add_argument("--reuse-db", action="store_true",
                   help="reuse the existing copy in --work-dir instead of taking a fresh one")
    args = p.parse_args()

    binary = args.binary.resolve()
    if not binary.exists():
        print(f"missing {binary}; run: cargo build --release --bin webspec-index", file=sys.stderr)
        return 2
    source = args.source_db.expanduser().resolve()

    version = subprocess.run([binary, "--version"], capture_output=True, text=True,
                             env=dict(os.environ, MOZTOOLS_UPDATE_CHECK="0")).stdout.split()[-1]
    source_info = db_info(source)
    if source_info["index_version"] != version:
        # Opening a DB indexed by another version purges it; the copy would be
        # emptied and every lookup would hit the network.
        print(f"index version {source_info['index_version']} != binary {version}; "
              "rebuild the index or the binary first", file=sys.stderr)
        return 2

    workloads = [
        w for w in WORKLOADS
        if (not args.only or any(o in w.name for o in args.only))
        and not (args.skip_indexing and w.group.startswith("indexing"))
        and (args.full or w.group != "indexing-full")
        and not (args.skip_runtime and w.group in ("runtime", "startup"))
        and (args.network or not w.network_only)
        and not (args.skip_indexing and w.group == "query-change")
    ]

    if args.reuse_db and (args.work_dir / "index.db").exists():
        db_path = args.work_dir / "index.db"
    else:
        print(f"copying {source} -> {args.work_dir}/index.db", file=sys.stderr)
        db_path = prepare_db(source, args.work_dir)

    base_env = dict(os.environ, SPEC_INDEX_TEST_DB=str(db_path), MOZTOOLS_UPDATE_CHECK="0")
    guard = network_guard()
    if not guard:
        print("warning: unshare -rn unavailable; running without a network guard", file=sys.stderr)

    stub_workloads = [w for w in workloads if w.group == "query-change"]
    normal_workloads = [w for w in workloads if w.group != "query-change"]

    results = []
    with tempfile.TemporaryDirectory() as tmp_str:
        tmp = Path(tmp_str)

        for w in normal_workloads:
            indexing = w.group.startswith("indexing") or w.group == "network"
            runs = w.runs or (args.indexing_runs if indexing else args.runs)
            warmup = w.warmup if w.warmup is not None else (
                args.indexing_warmup if indexing else args.warmup)
            cmd_guard = [] if w.no_guard else guard
            res = Result(w)
            fresh_dir = args.work_dir / f"fresh-{w.name}"
            for i in range(warmup + runs):
                if w.fresh_db or w.clear_memo:
                    cur_db = prepare_db(source, fresh_dir)
                    if w.clear_memo:
                        clear_memo_table(cur_db)
                    run_env = dict(os.environ, SPEC_INDEX_TEST_DB=str(cur_db),
                                   MOZTOOLS_UPDATE_CHECK="0")
                else:
                    run_env = base_env
                cmd = cmd_guard + [str(binary)] + w.args
                s = run_once(cmd, run_env, tmp)
                if i >= warmup:
                    res.samples.append(s)
            st = res.stats()
            results.append(st)
            print(f"{w.name:40} median {fmt_ms(st['wall_ms']['median']):>10}  "
                  f"p95 {fmt_ms(st['wall_ms']['p95']):>10}  rss {st['maxrss_kb_max'] / 1024:.0f} MB"
                  + ("" if st["exit_codes"] == [0] else f"  EXIT {st['exit_codes']}"),
                  file=sys.stderr)

        if stub_workloads:
            source_html = source.parent / "html"
            try:
                stub = StubServer(source_html)
            except FileNotFoundError as exc:
                print(f"warning: skipping query-change workloads: {exc}", file=sys.stderr)
                stub = None
            if stub is not None:
                stub_chain_warmup = max(
                    (w.warmup if w.warmup is not None else args.indexing_warmup)
                    for w in stub_workloads
                )
                stub_chain_runs = max(
                    (w.runs or args.indexing_runs) for w in stub_workloads
                )
                stub_results = {w.name: Result(w) for w in stub_workloads}
                stub_work_dir = args.work_dir / "stub"
                print(f"starting stub server at {stub.origin}", file=sys.stderr)
                for i in range(stub_chain_warmup + stub_chain_runs):
                    stub_db = prepare_stub_db(source, stub_work_dir)
                    stub_env = dict(
                        os.environ,
                        SPEC_INDEX_TEST_DB=str(stub_db),
                        MOZTOOLS_UPDATE_CHECK="0",
                        WEBSPEC_FETCH_ORIGIN=stub.origin,
                    )
                    for w in stub_workloads:
                        cmd = [str(binary)] + w.args
                        s = run_once(cmd, stub_env, tmp)
                        if i >= stub_chain_warmup:
                            stub_results[w.name].samples.append(s)
                stub.stop()
                for w in stub_workloads:
                    st = stub_results[w.name].stats()
                    results.append(st)
                    print(f"{w.name:40} median {fmt_ms(st['wall_ms']['median']):>10}  "
                          f"p95 {fmt_ms(st['wall_ms']['p95']):>10}  rss {st['maxrss_kb_max'] / 1024:.0f} MB"
                          + ("" if st["exit_codes"] == [0] else f"  EXIT {st['exit_codes']}"),
                          file=sys.stderr)

    now = dt.datetime.now().astimezone()
    report = {
        "meta": {
            "date": now.isoformat(timespec="seconds"),
            "git_sha": git("rev-parse", args.commit or "HEAD"),
            "git_dirty": False if args.commit else bool(
                git("status", "--porcelain", "--untracked-files=no")),
            "binary": str(binary),
            "binary_mtime": dt.datetime.fromtimestamp(binary.stat().st_mtime).astimezone()
                .isoformat(timespec="seconds"),
            "binary_version": version,
            "binary_bytes": binary.stat().st_size,
            "machine": machine_info(),
            "db": source_info,
            "network_guard": guard,
            "runtime_runs": args.runs,
            "runtime_warmup": args.warmup,
            "indexing_runs": args.indexing_runs,
            "indexing_warmup": args.indexing_warmup,
        },
        "results": results,
    }
    args.out_dir.mkdir(parents=True, exist_ok=True)
    stem = f"{now.strftime('%Y%m%d-%H%M%S')}-{report['meta']['git_sha'][:8]}"
    json_path = args.out_dir / f"{stem}.json"
    md_path = args.out_dir / f"{stem}.md"
    json_path.write_text(json.dumps(report, indent=2) + "\n")
    md_path.write_text(markdown(report))
    print(f"wrote {json_path}\nwrote {md_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
