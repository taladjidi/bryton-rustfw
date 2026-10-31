#!/usr/bin/env python3
"""Per-branch review report: python3 -I tools/review.py [--quick] [--base REF] [--jobs N] [--timeout S]; `selftest`.

What reviewers checked by hand, as a tool (check.sh runs only its selftest; the report is run per branch):
  mutants  cargo-mutants (config .cargo/mutants.toml) on `git diff BASE...HEAD` only: a MISSED mutant is a change in
           the branch's code that no host test notices. TIMEOUT mutants are listed but do not fail the exit code.
           The unmutated baseline must pass first. --quick runs them only on the changed files under src/storage/
           (the recorder, FAT, file writes); with no such change there are no mutants.
  clones   jscpd (jscpd-rs) on src/ and tools/ at BASE and at HEAD: clones at HEAD whose fragment text is not
           among BASE's are new copied code. Lines shift, so clones are matched by files and text, not numbers.
           A jscpd run that fails at BASE or HEAD is a failure.
  lint     tools/lint.py output.
The report goes to stdout and target/review/report.md. Exit 1 on a MISSED mutant, a new clone or a tool that failed
(cargo-mutants, jscpd, the worktree), else 0. The runner runs `--quick` before each merge; the full run is the review step.
Limits: jscpd-rs reports lines and the fragment, not tokens (its "tokens" field is 0): sizes are in lines. A clone
is old when its fragment text exists at BASE, in any file pair. Needs cargo-mutants and jscpd on PATH.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "target" / "review"
JSCPD = ["--min-lines", "5", "--min-tokens", "50", "-i", "**/fonts.rs,**/test-data/**,**/target/**", "-s", "--noTips"]
# cargo-mutants exit codes: 0 all caught, 2 missed mutants, 3 timeouts; 4 baseline failed, others usage or internal.
MUTANT_OK = (0, 2, 3)


def sh(cmd, cwd=ROOT):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)


def mutant_lines(text):
    """Non-empty lines of cargo-mutants' missed.txt / timeout.txt."""
    return [ln.strip() for ln in text.splitlines() if ln.strip()]


def quick_files(names):
    """The changed Rust files under src/storage/, the part --quick runs mutants on."""
    return [n for n in names if n.startswith("src/storage/") and n.endswith(".rs")]


def mutant_report(o, missed, timeout):
    """cargo-mutants' outcomes.json dict and its missed / timeout text as (report lines, count that fails)."""
    n = {k: o.get(k, 0) for k in ("caught", "missed", "unviable", "timeout")}
    lines = ["caught %(caught)d, missed %(missed)d, unviable %(unviable)d, timeout %(timeout)d" % n]
    lines += ["MISSED " + m for m in mutant_lines(missed)] + ["TIMEOUT " + m for m in mutant_lines(timeout)]
    return lines, n["missed"]


def clone_keys(report):
    """Clones of a jscpd JSON report as (sorted file pair, whitespace-free fragment) -> lines."""
    keys = {}
    for d in report.get("duplicates", []):
        files = tuple(sorted((d["firstFile"]["name"], d["secondFile"]["name"])))
        frag = "".join(d.get("fragment", "").split())
        keys[(files, frag)] = max(keys.get((files, frag), 0), d.get("lines", 0))
    return keys


def new_clones(head, base):
    """Clones of HEAD whose text is absent at BASE, as [(files, lines)]."""
    old = {frag for _, frag in base}
    return sorted((f, n) for (f, frag), n in head.items() if frag not in old)


def run_jscpd(path, tag):
    """The jscpd report as a dict, or None when jscpd exited non-zero or wrote no report."""
    out = OUT / ("jscpd-" + tag)
    shutil.rmtree(out, ignore_errors=True)
    r = sh(["jscpd", "-r", "json", "-o", str(out), *JSCPD, "src", "tools"], cwd=path)
    f = out / "jscpd-report.json"
    return json.loads(f.read_text()) if r.returncode == 0 and f.exists() else None


def mutants(base, jobs, timeout, quick):
    """(report lines, count that fails the review)."""
    diff = sh(["git", "diff", f"{base}...HEAD", "--", "*.rs"]).stdout
    if not diff.strip():
        return ["no Rust change in the diff: no mutants."], 0
    files = []
    if quick:
        files = quick_files(sh(["git", "diff", "--name-only", f"{base}...HEAD"]).stdout.splitlines())
        if not files:
            return ["--quick: no change under src/storage/: no mutants."], 0
    d = OUT / "mutants"
    shutil.rmtree(d, ignore_errors=True)
    d.mkdir(parents=True)
    (OUT / "diff.patch").write_text(diff)
    cmd = ["cargo", "mutants", "--in-diff", str(OUT / "diff.patch"), "--output", str(d), "--jobs", str(jobs),
           "--timeout", str(timeout), "--no-shuffle", *[x for f in files for x in ("--file", f)]]
    r = sh(cmd)
    if r.returncode not in MUTANT_OK:
        return [f"mutants FAILED (cargo-mutants exit {r.returncode}):", *r.stderr.splitlines()[-15:]], 1
    res = d / "mutants.out"
    if not (res / "outcomes.json").exists():
        return ["mutants FAILED to run (no outcomes.json):", *r.stderr.splitlines()[-15:], *r.stdout.splitlines()[-15:]], 1

    def rd(name):
        f = res / name
        return f.read_text() if f.exists() else ""

    return mutant_report(json.loads((res / "outcomes.json").read_text()), rd("missed.txt"), rd("timeout.txt"))


def main(argv):
    base, jobs, timeout, quick = "origin/main", max(1, (os.cpu_count() or 2) // 2), 300, False
    args = iter(argv)
    for a in args:
        if a == "--base":
            base = next(args)
        elif a == "--jobs":
            jobs = int(next(args))
        elif a == "--timeout":
            timeout = int(next(args))
        elif a == "--quick":
            quick = True
        else:
            sys.exit(__doc__)
    OUT.mkdir(parents=True, exist_ok=True)
    rep, bad = [f"# Review report: {base}...HEAD", ""], 0

    mlines, mbad = mutants(base, jobs, timeout, quick)
    rep += ["## Mutants", *("- " + m for m in mlines), ""]
    bad += mbad

    wt = Path(tempfile.mkdtemp(prefix="review-base-"))
    shutil.rmtree(wt)
    try:
        r = sh(["git", "worktree", "add", "--detach", str(wt), base])
        if r.returncode:
            rep += ["## Clones", "- git worktree failed: " + r.stderr.strip(), ""]
            bad += 1
        else:
            head, base_rep = run_jscpd(ROOT, "head"), run_jscpd(wt, "base")
            failed = [t for t, rp in (("HEAD", head), ("BASE", base_rep)) if rp is None]
            if failed:
                rep += ["## Clones", "- jscpd FAILED at " + " and ".join(failed), ""]
                bad += len(failed)
            else:
                new = new_clones(clone_keys(head), clone_keys(base_rep))
                rep += ["## New clones (jscpd, lines)", *(f"- {n} lines: {' <-> '.join(f)}" for f, n in new)]
                rep += ["- none"] if not new else []
                rep += [""]
                bad += len(new)
    finally:
        sh(["git", "worktree", "remove", "--force", str(wt)])
    lint = sh(["python3", "-I", "tools/lint.py"])
    rep += ["## Lint", (lint.stdout + lint.stderr).strip() or "clean", ""]
    text = "\n".join(rep)
    (OUT / "report.md").write_text(text)
    print(text)
    return 1 if bad else 0


def selftest():
    assert mutant_lines("a\n\n b \n") == ["a", "b"]
    o = {"caught": 3, "missed": 1, "timeout": 1, "unviable": 2}
    lines, bad = mutant_report(o, "src/a.rs:1: x\n", "src/b.rs:2: y\n")
    assert bad == 1
    assert lines == ["caught 3, missed 1, unviable 2, timeout 1", "MISSED src/a.rs:1: x", "TIMEOUT src/b.rs:2: y"]
    assert mutant_report({"timeout": 5}, "", "t\n")[1] == 0  # a timeout does not fail the review
    assert mutant_report({"caught": 4, "unviable": 1}, "", "") == (["caught 4, missed 0, unviable 1, timeout 0"], 0)
    assert quick_files(["src/storage/fat.rs", "src/ui/x.rs", "src/storage/notes.md", "src/storage/fit/a.rs"]) == [
        "src/storage/fat.rs", "src/storage/fit/a.rs"]
    assert quick_files(["src/ui/x.rs"]) == []
    rep = {"duplicates": [{"lines": 8, "firstFile": {"name": "b.rs"}, "secondFile": {"name": "a.rs"}, "fragment": "x y\n z"}]}
    k = clone_keys(rep)
    assert k == {(("a.rs", "b.rs"), "xyz"): 8}
    assert new_clones(k, k) == []
    assert new_clones(k, {}) == [(("a.rs", "b.rs"), 8)]
    print("review selftest ok")


if __name__ == "__main__":
    if sys.argv[1:] == ["selftest"]:
        selftest()
    else:
        sys.exit(main(sys.argv[1:]))
