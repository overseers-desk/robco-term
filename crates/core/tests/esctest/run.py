#!/usr/bin/env python3
"""Run esctest against the headless harness and compare with the pass-list.

The baseline is a list of test names that pass today (`passlist.txt`). The
check is one-directional on purpose: a test that stops passing is a
regression and fails this script, while a test that starts passing is good
news and only prints a line asking for the list to be updated. That
asymmetry is what makes the file a ratchet rather than a lock.

Usage:

    python3 crates/core/tests/esctest/run.py [--esctest DIR] [--update]

`DIR` is the directory holding esctest.py (default `~/code/esctest/esctest`).
The harness binary is taken from CARGO_TARGET_DIR or ./target, debug then
release; `--harness PATH` overrides it.
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
PASSLIST = os.path.join(HERE, "passlist.txt")
GAPS = os.path.join(HERE, "known-gaps.txt")

# The families a glyph-grid baseline cares about: cursor movement, erase,
# insert/delete, scrolling and the scroll region. Everything a renderer can
# disagree about lands on the screen through one of these.
INCLUDE = (
    r"^(CUP|CUU|CUD|CUF|CUB|CHA|VPA|HPA|HVP|CR|LF|BS|IND|RI|NEL|ED|EL|ECH"
    r"|ICH|DCH|IL|DL|SU|SD|DECSTBM|DECALN|DECSC|DECRC|CBT|CHT|HTS|TBC|REP)Tests\."
)

COLS = 80
ROWS = 25

# Which xterm's reverse-wraparound this core implements. esctest's default
# (0) means pre-380 xterm, where private mode 45 wrapped backwards across
# any line and around the screen; that reading makes two of its own tests
# contradict each other (`test_BS_WrapsInWraparoundMode` wants the wrap,
# `test_BS_InitialReverseWraparound` forbids the same wrap one row up).
# From patch 383 the two behaviours are separate modes -- 45 stays inside
# the logical line, 1045 wraps around -- and the suite is consistent. The
# core implements both, so it declares 383 and is measured against it.
XTERM_REVERSE_WRAP = "383"

def find_harness(explicit):
    if explicit:
        return explicit
    root = os.environ.get("CARGO_TARGET_DIR")
    if not root:
        root = os.path.join(HERE, "..", "..", "..", "..", "target")
    for profile in ("debug", "release"):
        path = os.path.abspath(os.path.join(root, profile, "esctest-harness"))
        if os.path.exists(path):
            return path
    sys.exit("esctest-harness not built: cargo build -p robco-core --bin esctest-harness")


def run(harness, esctest_dir, logfile):
    subprocess.run(
        [
            harness,
            "--cols", str(COLS),
            "--rows", str(ROWS),
            "--",
            sys.executable,
            os.path.join(esctest_dir, "esctest.py"),
            "--expected-terminal", "xterm",
            "--xterm-reverse-wrap", XTERM_REVERSE_WRAP,
            "--include", INCLUDE,
            "--logfile", logfile,
        ],
        check=False,
        # The child talks to the PTY, not to us; anything on our stdout would
        # be the harness's own noise.
        stdout=subprocess.DEVNULL,
    )
    with open(logfile, "rb") as f:
        return f.read().decode("utf-8", "replace")


def parse(log):
    """Return (passed, failed, known_bugs, summary_line)."""
    passed, failed, known = [], [], []
    current = None
    for line in log.splitlines():
        m = re.match(r"Run test: (\S+)", line)
        if m:
            current = m.group(1)
            continue
        if current is None:
            continue
        if line.startswith("Passed."):
            passed.append(current)
            current = None
        elif line.startswith("Fails as expected:"):
            known.append(current)
            current = None
        elif line.startswith("*** TEST ") and "FAILED" in line:
            failed.append(current)
            current = None
        elif line.startswith("Skipped because"):
            current = None
    summary = ""
    for line in log.splitlines():
        if "tests passed" in line or "test passed" in line:
            summary = line.strip()
    return sorted(passed), sorted(failed), sorted(known), summary


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--esctest", default=os.path.expanduser("~/code/esctest/esctest"))
    ap.add_argument("--harness", default=None)
    ap.add_argument("--update", action="store_true",
                    help="Rewrite passlist.txt from this run.")
    args = ap.parse_args()

    harness = find_harness(args.harness)
    if not os.path.exists(os.path.join(args.esctest, "esctest.py")):
        sys.exit("esctest.py not found in %s (pass --esctest DIR)" % args.esctest)

    with tempfile.NamedTemporaryFile(suffix=".log") as tmp:
        log = run(harness, args.esctest, tmp.name)
    passed, failed, known, summary = parse(log)

    print("esctest: %s" % (summary or "no summary line"))
    print("  passed %d, failed %d, known bugs %d" % (len(passed), len(failed), len(known)))

    if args.update:
        with open(PASSLIST, "w") as f:
            f.write("# esctest tests that pass against the RobCo headless harness.\n")
            f.write("# Regenerate: python3 crates/core/tests/esctest/run.py --update\n")
            f.write("# Include regex: %s\n" % INCLUDE)
            f.write("# Geometry: %dx%d\n" % (COLS, ROWS))
            f.write("# xterm reverse-wrap generation: %s\n" % XTERM_REVERSE_WRAP)
            for name in passed:
                f.write(name + "\n")
        print("wrote %s (%d tests)" % (PASSLIST, len(passed)))
        with open(GAPS, "w") as f:
            f.write("# esctest tests that do NOT pass, recorded so the gaps are\n")
            f.write("# visible rather than merely absent from the pass-list.\n")
            f.write("# Regenerate: python3 crates/core/tests/esctest/run.py --update\n")
            for name in failed:
                f.write(name + "\n")
            for name in known:
                f.write("%s  # esctest expects this to fail on xterm too\n" % name)
        print("wrote %s (%d tests)" % (GAPS, len(failed) + len(known)))
        return 0

    if not os.path.exists(PASSLIST):
        sys.exit("no pass-list at %s; run with --update to create it" % PASSLIST)
    with open(PASSLIST) as f:
        baseline = sorted(l.strip() for l in f if l.strip() and not l.startswith("#"))

    now = set(passed)
    regressed = [n for n in baseline if n not in now]
    gained = sorted(now - set(baseline))

    for name in gained:
        print("  NEW PASS  %s (add it to passlist.txt)" % name)
    for name in regressed:
        print("  REGRESSED %s" % name)
    if regressed:
        print("FAIL: %d test(s) that used to pass no longer do." % len(regressed))
        return 1
    print("OK: all %d baseline tests still pass." % len(baseline))
    return 0


if __name__ == "__main__":
    sys.exit(main())
