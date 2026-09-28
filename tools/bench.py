#!/usr/bin/env python3
"""Native vs record vs replay: wall time, CPU time, peak RSS and trace size.

    cargo build --release -p retrace
    tools/bench.py [--runs N] [--retrace PATH]

Signs a copy of the release binary with `retrace.entitlements` (the cargo runner does this for
`cargo run`; a binary run by hand needs it too), runs every workload once untimed (the first run
of a freshly signed binary can stall in codesign validation for minutes, and would otherwise be
measured as retrace), then N timed runs of each phase, and prints the median of each. Record and
replay must both exit with the native exit code or the workload is reported as failed, never
timed. Workloads that need Homebrew (`jq`, `python@3.14`) are skipped with a line that says so.
"""
import argparse
import json
import os
import random
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
JQ = "/opt/homebrew/bin/jq"
PY = ("/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/"
      "Contents/MacOS/Python")


def workloads(data):
    return [
        ("echo", "/bin/echo", ["hi"]),
        ("ls /usr/bin", "/bin/ls", ["/usr/bin"]),
        ("jq, 5 MB file", JQ, ["-c", "map(select(.score > 0.5)) | length", data]),
        ("jq, compute", JQ, ["-n", "[range(0;100000)] | add"]),
        ("python -c 'print(1)'", PY, ["-c", "print(1)"]),
        ("python, 30M-step loop", PY, ["-c", "print(sum(i*i for i in range(30_000_000)))"]),
    ]


def run(argv, cwd):
    """One run: (exit code, wall s, cpu s, peak rss bytes)."""
    t0 = time.perf_counter()
    p = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, status, ru = os.wait4(p.pid, 0)
    wall = time.perf_counter() - t0
    rc = os.waitstatus_to_exitcode(status)
    return rc, wall, ru.ru_utime + ru.ru_stime, ru.ru_maxrss  # ru_maxrss is bytes on macOS


def median_of(argv, cwd, n):
    rs = [run(argv, cwd) for _ in range(n)]
    rcs = {r[0] for r in rs}
    return (rcs.pop() if len(rcs) == 1 else None,
            statistics.median(r[1] for r in rs),
            statistics.median(r[2] for r in rs),
            max(r[3] for r in rs))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--retrace",
                    default=os.path.join(REPO, "target/aarch64-apple-darwin/release/retrace"))
    a = ap.parse_args()

    work = tempfile.mkdtemp(prefix="retrace-bench-")
    rt = os.path.join(work, "retrace")
    shutil.copy(a.retrace, rt)
    subprocess.run(["codesign", "-s", "-", "-f", "--entitlements",
                    os.path.join(REPO, "retrace.entitlements"), rt],
                   check=True, capture_output=True)
    data = os.path.join(work, "data.json")
    rng = random.Random(1)
    with open(data, "w") as f:
        json.dump([{"id": i, "name": f"item{i}", "tags": ["a", "b", "c"][: i % 4],
                    "score": rng.random(), "nested": {"x": i * 3, "y": [i, i + 1, i + 2]}}
                   for i in range(40000)], f)

    print(f"runs={a.runs} (median; peak RSS is the max over runs)")
    hdr = ("workload", "native", "record", "replay", "rec x", "rep x", "rec RSS", "trace")
    print("| " + " | ".join(hdr) + " |")
    print("|" + "---|" * len(hdr))
    for name, exe, args in workloads(data):
        if not os.path.exists(exe):
            print(f"SKIPPED {name}: {exe} not found")
            continue
        trace = os.path.join(work, "t.bin")
        native = [exe, *args]
        record = [rt, "record-dyn", exe, "-o", trace, "--", *args]
        replay = [rt, "replay", trace]
        for argv in (native, record, replay):  # untimed warm-up
            run(argv, work)
        n = median_of(native, work, a.runs)
        r = median_of(record, work, a.runs)
        p = median_of(replay, work, a.runs)
        if not (n[0] == r[0] == p[0]):
            print(f"| {name} | FAILED: exit codes native={n[0]} record={r[0]} replay={p[0]} |")
            continue
        size = os.path.getsize(trace)
        print(f"| {name} | {n[1]:.3f} s | {r[1]:.3f} s | {p[1]:.3f} s | "
              f"{r[1] / n[1]:.1f}x | {p[1] / n[1]:.1f}x | "
              f"{r[3] / 2**20:.0f} MiB | {size / 2**20:.0f} MiB |")
    shutil.rmtree(work)


if __name__ == "__main__":
    sys.exit(main())
