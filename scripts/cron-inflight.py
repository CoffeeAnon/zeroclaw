#!/usr/bin/env python3
"""cron-inflight — operator triage for in-flight Sam cron sessions.

Differentiates between LLM wait, verbose reasoning, scheduler-internal
errors, and silent crashes — failure modes that today look identical
from outside (no `cron_runs` row, `last_run` null, generic silence).

Reads:
  - cron_jobs + cron_runs from Sam's PVC sqlite via `kubectl exec`
  - Sam's pod logs via `kubectl logs --since=N --timestamps`

For each enabled cron whose `next_run` is in the past with no completed
run since, classifies the in-flight session into one of:

  responsive            — Reasoning: line within last 30s
  verbose-reasoning     — many turns in a 2-min window, no provider WARN
  llm-stalled           — provider WARN within last 60s, no reasoning since
  silent-suspect-crash  — no log activity for >5 min
  scheduler-error       — scheduler WARN/ERROR matched
  slow                  — progressing but neither responsive nor verbose
  starting              — just fired, no log evidence yet
  completed             — already recorded a run after this fire window
  not-due               — next_run still in the future

With `--recent-completed N`, ALSO checks every cron_runs entry from the
last N hours for post-fire artifact health (Fix 1b — catches the case
where status=ok but the intended side effect didn't happen):

  verified-success      — status=ok with at least one wiki-tree file
                          OR memory-DB file modified during the run
                          window, OR a short duration where no artifact
                          write was expected
  silent-success        — status=ok, ran >60s, but no wiki-tree or
                          memory-DB changes detected during the run
                          window. Suspicious — this is the failure mode
                          where Sam reasons her way through Step N but
                          never persists the intended write (e.g. final
                          assistant message is a thought without a tool
                          call).
  failed-run            — status=error

Exit codes:
  0  no stalls detected (responsive/completed/not-due/starting/
     verified-success only)
  2  one or more jobs in scheduler-error/llm-stalled/silent-suspect-crash/
     verbose-reasoning/slow/silent-success/failed-run state
  3  kubectl unreachable / pod inaccessible
  4  invalid args (e.g. --job names a cron that doesn't exist)
"""

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
from typing import Any, Dict, List, Optional, Tuple

NAMESPACE = "ai-agents"
POD = "zeroclaw"
CONTAINER = "zeroclaw"

LLM_STALL_RECENT_S = 60
RESPONSIVE_RECENT_S = 30
SILENT_SUSPECT_CRASH_S = 300
VERBOSE_MIN_TURNS = 5
VERBOSE_WINDOW_S = 120

# Fix 1b — post-fire artifact thresholds
SILENT_SUCCESS_MIN_DURATION_S = 60      # below this, no-write is presumed legitimate
WIKI_ROOT = "/data/workspace/wiki/"     # tree to check for post-fire modifications
WIKI_PROBE_GRACE_S = 30                 # accept mtimes up to N seconds after finished_at
# Sam's persistent memory store. SQLite WAL mode means writes hit `-wal`
# first and `.db` only on checkpoint, so we track both. Some skills
# (self-reflection-check) write to memory but not the wiki — without
# this probe they false-positive as silent-success.
MEMORY_DB_FILES = [
    "/data/.zeroclaw/workspace/memory/brain.db",
    "/data/.zeroclaw/workspace/memory/brain.db-wal",
]

# kubectl prefixes each log line with an RFC3339 timestamp, then a space, then
# whatever the container wrote. Sam's own log lines often embed *another*
# timestamp inside ANSI escape sequences. We always anchor on the kubectl
# timestamp (first whitespace-delimited token), not the inner one.
KUBECTL_TS_RX = re.compile(r"^(\S+)\s+(.*)$")
REASON_RX = re.compile(r"Reasoning:")
PROVIDER_WARN_RX = re.compile(r"providers::reliable.*Provider call failed")
PROVIDER_RECOVER_RX = re.compile(r"providers::reliable.*Provider recovered")
SCHED_ERROR_RX = re.compile(r"(Failed to persist scheduler|Invalid cron expression)")


def utcnow() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc)


def parse_iso(s: Optional[str]) -> Optional[dt.datetime]:
    if not s:
        return None
    s = s.strip().replace("Z", "+00:00")
    try:
        return dt.datetime.fromisoformat(s).astimezone(dt.timezone.utc)
    except ValueError:
        return None


def kubectl_exec_python(code: str) -> Tuple[int, str, str]:
    try:
        out = subprocess.run(
            ["kubectl", "exec", "-n", NAMESPACE, POD, "-c", CONTAINER, "--", "python3", "-c", code],
            capture_output=True, text=True, timeout=30,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
        print(f"error: kubectl exec failed: {exc}", file=sys.stderr)
        sys.exit(3)
    return out.returncode, out.stdout, out.stderr


def kubectl_logs(since: str) -> str:
    try:
        out = subprocess.run(
            ["kubectl", "logs", "-n", NAMESPACE, POD, "-c", CONTAINER,
             f"--since={since}", "--timestamps"],
            capture_output=True, text=True, timeout=60,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
        print(f"error: kubectl logs failed: {exc}", file=sys.stderr)
        sys.exit(3)
    if out.returncode != 0:
        print(f"error: kubectl logs returned {out.returncode}: {out.stderr}", file=sys.stderr)
        sys.exit(3)
    return out.stdout


def fetch_jobs() -> List[Dict[str, Any]]:
    code = (
        "import sqlite3, json\n"
        "c = sqlite3.connect('file:/data/.zeroclaw/workspace/cron/jobs.db?mode=ro', uri=True)\n"
        "c.row_factory = sqlite3.Row\n"
        "out = [dict(r) for r in c.execute("
        "  'SELECT id, name, next_run, last_run, last_status FROM cron_jobs WHERE enabled=1')]\n"
        "print(json.dumps(out))\n"
    )
    rc, stdout, stderr = kubectl_exec_python(code)
    if rc != 0:
        print(f"error: cron_jobs query failed: {stderr}", file=sys.stderr)
        sys.exit(3)
    return json.loads(stdout.strip()) if stdout.strip() else []


def fetch_recent_runs(lookback_hours: int) -> List[Dict[str, Any]]:
    """Return cron_runs entries from the last N hours, joined with job_name."""
    code = (
        "import sqlite3, json\n"
        "c = sqlite3.connect('file:/data/.zeroclaw/workspace/cron/jobs.db?mode=ro', uri=True)\n"
        "c.row_factory = sqlite3.Row\n"
        f"q = ('SELECT r.job_id, r.started_at, r.finished_at, r.status, r.duration_ms, "
        "j.name AS job_name FROM cron_runs r "
        "JOIN cron_jobs j ON j.id = r.job_id "
        f"WHERE datetime(r.started_at) >= datetime(\"now\", \"-{int(lookback_hours)} hours\") "
        "ORDER BY r.started_at DESC')\n"
        "out = [dict(r) for r in c.execute(q)]\n"
        "print(json.dumps(out))\n"
    )
    rc, stdout, stderr = kubectl_exec_python(code)
    if rc != 0:
        print(f"error: cron_runs query failed: {stderr}", file=sys.stderr)
        sys.exit(3)
    return json.loads(stdout.strip()) if stdout.strip() else []


def probe_artifact_changes(started_at: str, finished_at: str) -> Dict[str, List[str]]:
    """Enumerate files whose mtime falls within the run window plus
    WIKI_PROBE_GRACE_S after finished_at. Returns {"wiki": [...], "memory": [...]}.

    Combines a tree walk under WIKI_ROOT and a stat() pass over MEMORY_DB_FILES
    into a single kubectl exec to avoid two round-trips."""
    started = parse_iso(started_at)
    finished = parse_iso(finished_at)
    if not started or not finished:
        return {"wiki": [], "memory": []}
    start_ts = started.timestamp()
    end_ts = finished.timestamp() + WIKI_PROBE_GRACE_S
    code = (
        "import os, json\n"
        f"start_ts = {start_ts}\n"
        f"end_ts = {end_ts}\n"
        f"root = {WIKI_ROOT!r}\n"
        f"memory_files = {MEMORY_DB_FILES!r}\n"
        "wiki = []\n"
        "for d, _, files in os.walk(root):\n"
        "    for f in files:\n"
        "        p = os.path.join(d, f)\n"
        "        try:\n"
        "            m = os.path.getmtime(p)\n"
        "            if start_ts <= m <= end_ts:\n"
        "                wiki.append(p)\n"
        "        except OSError:\n"
        "            pass\n"
        "memory = []\n"
        "for p in memory_files:\n"
        "    try:\n"
        "        m = os.path.getmtime(p)\n"
        "        if start_ts <= m <= end_ts:\n"
        "            memory.append(p)\n"
        "    except OSError:\n"
        "        pass\n"
        "print(json.dumps({'wiki': wiki, 'memory': memory}))\n"
    )
    rc, stdout, _ = kubectl_exec_python(code)
    if rc != 0:
        return {"wiki": [], "memory": []}
    try:
        result = json.loads(stdout.strip()) if stdout.strip() else {}
        return {
            "wiki": result.get("wiki", []),
            "memory": result.get("memory", []),
        }
    except json.JSONDecodeError:
        return {"wiki": [], "memory": []}


def diagnose_post_fire(run: Dict[str, Any], artifacts: Dict[str, List[str]]) -> Dict[str, Any]:
    """Classify a completed cron_runs row using post-fire artifact evidence."""
    duration_s = (run.get("duration_ms") or 0) / 1000.0
    wiki_files = artifacts.get("wiki", [])
    memory_files = artifacts.get("memory", [])
    base = {
        "duration_seconds": duration_s,
        "wiki_modification_count": len(wiki_files),
        "memory_modification_count": len(memory_files),
        "wiki_files_modified_during_run": wiki_files[:10],  # cap to avoid blob output
        "memory_files_modified_during_run": memory_files,
    }
    if run.get("status") == "error":
        return {**base, "diagnosis": "failed-run"}
    if wiki_files or memory_files:
        return {**base, "diagnosis": "verified-success"}
    if duration_s < SILENT_SUCCESS_MIN_DURATION_S:
        return {**base, "diagnosis": "verified-success",
                "note": "short cron, no artifact write expected"}
    return {**base, "diagnosis": "silent-success",
            "note": "ran >60s with status=ok but no wiki-tree or memory-DB writes during run window — suspicious"}


def parse_log_events(logs: str) -> List[Tuple[str, dt.datetime, str]]:
    """Return [(kind, ts, line)] for relevant log signals.

    kind is one of: reasoning, provider_warn, provider_recover, sched_error.
    """
    events: List[Tuple[str, dt.datetime, str]] = []
    for line in logs.splitlines():
        m = KUBECTL_TS_RX.match(line)
        if not m:
            continue
        ts_str, body = m.group(1), m.group(2)
        ts = parse_iso(ts_str)
        if ts is None:
            continue
        # Order matters only for early-exit; a single line maps to one kind.
        if SCHED_ERROR_RX.search(body):
            events.append(("sched_error", ts, body))
        elif PROVIDER_WARN_RX.search(body):
            events.append(("provider_warn", ts, body))
        elif PROVIDER_RECOVER_RX.search(body):
            events.append(("provider_recover", ts, body))
        elif REASON_RX.search(body):
            events.append(("reasoning", ts, body))
    return events


def diagnose(
    job: Dict[str, Any],
    events: List[Tuple[str, dt.datetime, str]],
    now: dt.datetime,
) -> Dict[str, Any]:
    next_run = parse_iso(job["next_run"])
    last_run = parse_iso(job["last_run"])

    if next_run is None:
        return {"diagnosis": "unknown-no-next-run", "elapsed_seconds": None}
    if now < next_run:
        return {
            "diagnosis": "not-due",
            "elapsed_seconds": None,
            "seconds_until_next_run": (next_run - now).total_seconds(),
        }

    fire_window_start = next_run
    elapsed = (now - fire_window_start).total_seconds()

    # Already completed? cron_runs would be definitive but we only have
    # last_run on the job row; that's enough to say "fired and recorded".
    if last_run and last_run >= fire_window_start:
        return {"diagnosis": "completed", "elapsed_seconds": elapsed,
                "completed_at": last_run.strftime("%Y-%m-%dT%H:%M:%SZ")}

    # In flight — analyze events
    sched_errors = [e for e in events if e[0] == "sched_error"]
    if sched_errors:
        return {"diagnosis": "scheduler-error", "elapsed_seconds": elapsed,
                "evidence": sched_errors[-1][2][:240]}

    reasoning = [e for e in events if e[0] == "reasoning"]
    provider_warns = [e for e in events if e[0] == "provider_warn"]
    provider_recovers = [e for e in events if e[0] == "provider_recover"]

    relevant = reasoning + provider_warns + provider_recovers
    if not relevant:
        if elapsed > SILENT_SUSPECT_CRASH_S:
            return {"diagnosis": "silent-suspect-crash", "elapsed_seconds": elapsed,
                    "seconds_since_last_log": None,
                    "note": "next_run is in the past but no log activity in the window"}
        return {"diagnosis": "starting", "elapsed_seconds": elapsed}

    last_event_ts = max(e[1] for e in relevant)
    seconds_since_last_log = (now - last_event_ts).total_seconds()

    if provider_warns:
        last_warn = max(e[1] for e in provider_warns)
        last_recover = max((e[1] for e in provider_recovers), default=None)
        last_reasoning = max((e[1] for e in reasoning), default=None)
        warn_unrecovered = last_recover is None or last_warn > last_recover
        warn_more_recent_than_reasoning = last_reasoning is None or last_warn > last_reasoning
        if warn_unrecovered and warn_more_recent_than_reasoning and \
                (now - last_warn).total_seconds() < LLM_STALL_RECENT_S:
            return {"diagnosis": "llm-stalled", "elapsed_seconds": elapsed,
                    "seconds_since_last_log": seconds_since_last_log,
                    "evidence": provider_warns[-1][2][:240]}

    if seconds_since_last_log < RESPONSIVE_RECENT_S:
        return {"diagnosis": "responsive", "elapsed_seconds": elapsed,
                "seconds_since_last_log": seconds_since_last_log,
                "reasoning_turns_observed": len(reasoning)}

    if seconds_since_last_log > SILENT_SUSPECT_CRASH_S:
        return {"diagnosis": "silent-suspect-crash", "elapsed_seconds": elapsed,
                "seconds_since_last_log": seconds_since_last_log,
                "reasoning_turns_observed": len(reasoning)}

    recent_reasoning = [e for e in reasoning
                        if (now - e[1]).total_seconds() < VERBOSE_WINDOW_S]
    if len(recent_reasoning) >= VERBOSE_MIN_TURNS:
        return {"diagnosis": "verbose-reasoning", "elapsed_seconds": elapsed,
                "seconds_since_last_log": seconds_since_last_log,
                "reasoning_turns_observed": len(reasoning),
                "recent_turns_2m": len(recent_reasoning)}

    return {"diagnosis": "slow", "elapsed_seconds": elapsed,
            "seconds_since_last_log": seconds_since_last_log,
            "reasoning_turns_observed": len(reasoning)}


STALL_DIAGNOSES = {
    "scheduler-error", "llm-stalled", "silent-suspect-crash",
    "verbose-reasoning", "slow",
    # Fix 1b additions — completed runs with suspicious or failed outcomes
    "silent-success", "failed-run",
}


def main(argv: Optional[List[str]] = None) -> int:
    p = argparse.ArgumentParser(prog="cron-inflight", description=__doc__.split("\n\n")[0])
    p.add_argument("--since", default="30m",
                   help="kubectl logs --since window (default 30m)")
    p.add_argument("--job",
                   help="filter to one cron name (otherwise: all enabled crons)")
    p.add_argument("--text", action="store_true",
                   help="human-friendly text output instead of JSON")
    p.add_argument("--all", action="store_true",
                   help="include not-due and completed crons in output (default: stalls + responsive only)")
    p.add_argument("--recent-completed", type=int, default=0, metavar="HOURS",
                   help="also check cron_runs from the last HOURS for post-fire artifact health "
                        "(Fix 1b — catches status=ok with no observable side effect). 0 disables. "
                        "Typical: --recent-completed 1 right after a fire.")
    args = p.parse_args(argv)

    now = utcnow()
    jobs = fetch_jobs()

    if args.job:
        matched = [j for j in jobs if j["name"] == args.job]
        if not matched:
            print(f"error: no enabled cron named {args.job!r}", file=sys.stderr)
            return 4
        jobs = matched

    logs = kubectl_logs(args.since)
    events = parse_log_events(logs)

    diagnosed = []
    has_stall = False
    for job in jobs:
        d = diagnose(job, events, now)
        d.update({"name": job["name"], "job_id": job["id"],
                  "next_run": job["next_run"], "last_run": job["last_run"]})
        diagnosed.append(d)
        if d["diagnosis"] in STALL_DIAGNOSES:
            has_stall = True

    if not args.all:
        diagnosed = [d for d in diagnosed
                     if d["diagnosis"] not in ("not-due", "completed")]

    # Fix 1b: post-fire artifact-assertion checks for recently-completed runs
    post_fire = []
    if args.recent_completed > 0:
        runs = fetch_recent_runs(args.recent_completed)
        if args.job:
            runs = [r for r in runs if r["job_name"] == args.job]
        for run in runs:
            artifacts = probe_artifact_changes(run["started_at"], run["finished_at"])
            d = diagnose_post_fire(run, artifacts)
            d.update({
                "name": run["job_name"],
                "job_id": run["job_id"],
                "started_at": run["started_at"],
                "finished_at": run["finished_at"],
                "status": run["status"],
            })
            post_fire.append(d)
            if d["diagnosis"] in STALL_DIAGNOSES:
                has_stall = True

    payload = {
        "version": 1,
        "generated_at": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "log_window": args.since,
        "stall_diagnoses": sorted(STALL_DIAGNOSES),
        "jobs": diagnosed,
        "post_fire_runs": post_fire,
    }

    if args.text:
        if not diagnosed and not post_fire:
            print("no in-flight or stalled crons; all healthy")
        for j in diagnosed:
            elapsed = j.get("elapsed_seconds")
            elapsed_str = f"{elapsed:>6.0f}s" if elapsed is not None else "    n/a"
            since_log = j.get("seconds_since_last_log")
            since_log_str = f"{since_log:>5.0f}s" if since_log is not None else "  n/a"
            print(f"[{j['diagnosis']:<22}] {j['name']:<30} elapsed={elapsed_str} since_log={since_log_str}")
            if "evidence" in j:
                print(f"    evidence: {j['evidence']}")
            if j.get("reasoning_turns_observed") is not None:
                print(f"    turns_in_window={j['reasoning_turns_observed']}"
                      + (f"  recent_2m={j['recent_turns_2m']}" if "recent_turns_2m" in j else ""))
        if post_fire:
            print()
            print(f"--- post-fire artifact checks (last {args.recent_completed}h) ---")
            for r in post_fire:
                dur = r.get("duration_seconds", 0)
                print(f"[{r['diagnosis']:<22}] {r['name']:<30} duration={dur:>6.0f}s "
                      f"wiki_changes={r['wiki_modification_count']} "
                      f"memory_changes={r['memory_modification_count']}")
                if r.get("note"):
                    print(f"    note: {r['note']}")
                if r.get("wiki_files_modified_during_run"):
                    for p in r["wiki_files_modified_during_run"]:
                        print(f"    + wiki: {p}")
                if r.get("memory_files_modified_during_run"):
                    for p in r["memory_files_modified_during_run"]:
                        print(f"    + memory: {p}")
    else:
        print(json.dumps(payload, indent=2))

    return 2 if has_stall else 0


if __name__ == "__main__":
    sys.exit(main())
