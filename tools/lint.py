#!/usr/bin/env python3
"""Checks reviewers did by hand, run by check.sh (python3 -I tools/lint.py; `selftest` proves each fires).

Each hit prints `lint: <check>: file:line: what`; the exit status is 1 on any hit.
  unused-pub  a pub / pub(crate) item of src/ that only tests (or doc tests) use: make it #[cfg(test)] or ALLOW it
  dashes      U+2014 / U+2013 in a tracked text file (AGENTS.md, Wording)
  keys        a licensed key file's bytes (raw or hex) inside a tracked file; only the file name is printed
  tools-dead  a function of tools/ or renode/ that nothing names
  ruff        pyflakes errors (F, E9) of tools/ and renode/, when ruff is installed
"""
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# name -> why a pub item without a non-test use in src/ stays. A stale entry is a hit too.
ALLOW = {
    "broadcast": "src/drivers/fecble.rs: used by src/sim and by its own doc test",
    "readable": "src/ui/theme.rs: its doc test is an example that needs it public",
}
# the same for tools-dead
ALLOW_DEAD = {}
HOOKS = {"do_GET", "do_POST", "log_message", "main", "selftest"}
KEY_FILES = ("ant_key.bin", "di2_key.bin")
TEXT = (".rs", ".md", ".py", ".sh", ".toml", ".x", ".txt", ".json", ".ini")

# comments, strings, char literals (lifetimes have no closing quote): blanked, newlines kept
BLANK = re.compile(
    r"//[^\n]*|/\*.*?\*/|(?<![A-Za-z0-9_])b?r(#*)\".*?\"\1|b?\"(?:\\.|[^\"\\])*\"|b?'(?:\\.[^']*|[^\\'])'", re.S
)
ITEM = re.compile(
    r"^\s*(?:#\[[^\]]*\]\s*)*pub(?:\([a-z]+\))?\s+(?:(?:unsafe|const|async|extern)\s+)*"
    r"(fn|const|static|struct|enum|type|trait)\s+(?:mut\s+)?(\w+)"
)
DEF_BEFORE = re.compile(r"(?:fn|const|static|struct|enum|type|trait)\s+$")


def blank(text):
    return BLANK.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), text)


def files(root):
    if (root / ".git").exists():
        out = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True, check=True).stdout
        names = [n for n in out.decode().split("\0") if n]
    else:
        names = [str(p.relative_to(root)) for p in root.rglob("*") if p.is_file()]
    return sorted(n for n in names if (root / n).is_file())


def scan(code):
    """-> (items, nontest): items are (line, kind, name, in_trait_impl_or_trait, in_test); nontest is the code
    outside #[cfg(test)] blocks. One pass over the braces; a header is the text since the last { } or ;."""
    items, keep = [], []
    stack = []  # per open brace: "test" / "traitimpl" / "other"
    header, hline, paren, line = [], 1, 0, 1
    in_test = False

    def flush(gate=True):
        nonlocal header
        h = "".join(header)
        test = in_test or (gate and "#[cfg(test)]" in re.sub(r"\s+", "", h))
        m = ITEM.match(h)
        if m and not test:
            skip = any(k == "traitimpl" for k in stack)
            items.append((hline + h[: len(h) - len(h.lstrip())].count("\n"), m.group(1), m.group(2), skip))
        if not test:
            keep.append(h)
        header = []
        return h, test

    for ch in code:
        if ch == "\n":
            line += 1
        if ch in "([":
            paren += 1
        elif ch in ")]":
            paren -= 1
        if ch == "{":
            h, test = flush()
            hl = re.sub(r"\s+", " ", re.sub(r"#\[[^\]]*\]", "", h)).strip()
            if re.match(r"(unsafe )?impl\b.* for ", hl) or re.match(r"(pub(\([a-z]+\))? )?(unsafe )?trait\b", hl):
                kind = "traitimpl"
            else:
                kind = "other"
            stack.append("test" if test else kind)
            in_test = in_test or test
            hline = line
        elif ch == "}":
            flush(False)
            if stack:
                stack.pop()
            in_test = "test" in stack
            hline = line
        elif ch == ";" and paren <= 0:
            flush()
            hline = line
        else:
            if not header:
                hline = line
            header.append(ch)
            continue
        if not in_test:
            keep.append(ch)
    keep.append("".join(header))
    return items, "".join(keep)


def unused_pub(root):
    hits, per_file, uses = [], {}, {}
    for n in files(root):
        if not n.startswith("src/") or not n.endswith(".rs") or n.startswith("src/sim/") or n == "src/ui/fonts.rs":
            continue
        items, kept = scan(blank((root / n).read_text()))
        per_file[n] = items
        for m in re.finditer(r"\w+", kept):
            if not DEF_BEFORE.search(kept[max(0, m.start() - 12) : m.start()]):
                uses[m.group(0)] = uses.get(m.group(0), 0) + 1
    seen = set()
    for n, items in per_file.items():
        for line, kind, name, skip in items:
            if skip:
                continue
            if name in ALLOW:
                seen.add(name)
                continue
            if uses.get(name, 0) == 0:
                hits.append(f"lint: unused-pub: {n}:{line}: {kind} `{name}` has no use outside tests (cfg(test) it, or ALLOW)")
    for name in ALLOW:
        if name not in seen:
            hits.append(f"lint: unused-pub: tools/lint.py:0: stale ALLOW entry `{name}`")
    return hits


def dashes(root):
    hits = []
    for n in files(root):
        if n.endswith(TEXT):
            for i, l in enumerate((root / n).read_text(errors="replace").splitlines(), 1):
                if "\u2014" in l or "\u2013" in l:
                    hits.append(f"lint: dashes: {n}:{i}: em or en dash")
    return hits


def key_forms(k):
    h = k.hex()
    out = {k, h.encode(), h.upper().encode()}
    for sep in (b", 0x", b" ", b":", b"-", b",", b", "):
        for hx in (h, h.upper()):
            pairs = [hx[i : i + 2].encode() for i in range(0, len(hx), 2)]
            out.add(sep.join(pairs))
            out.add(b"0x" + sep.join(pairs) if sep.endswith(b"0x") else sep.join(b"0x" + p for p in pairs))
    return {f for f in out if len(f) >= 8}


def keys(root):
    found = [root / k for k in KEY_FILES if (root / k).is_file() and (root / k).stat().st_size >= 4]
    if not found:
        print("lint: keys: no key files, skipped")
        return []
    forms = set().union(*(key_forms(f.read_bytes()) for f in found))
    hits = []
    for n in files(root):
        if n in KEY_FILES:
            continue
        data = (root / n).read_bytes()
        if any(f in data for f in forms):
            hits.append(f"lint: keys: {n}:0: contains a licensed key")
    return hits


def tools_dead(root):
    pys = [n for n in files(root) if re.match(r"(tools|renode)/[^/]*\.py$", n)]
    corpus = "\n".join(
        (root / n).read_text(errors="replace")
        for n in files(root)
        if n.startswith(("tools/", "renode/", "docs/")) or n == "check.sh"
        if n.endswith(TEXT + (".resc", ".repl"))
    )
    counts = {}
    for w in re.findall(r"\w+", corpus):
        counts[w] = counts.get(w, 0) + 1
    hits, seen = [], set()
    for n in pys:
        for i, l in enumerate((root / n).read_text().splitlines(), 1):
            m = re.match(r"def (\w+)", l)
            if not m or m.group(1) in HOOKS:
                continue
            if m.group(1) in ALLOW_DEAD:
                seen.add(m.group(1))
            elif counts.get(m.group(1), 0) <= 1:
                hits.append(f"lint: tools-dead: {n}:{i}: `{m.group(1)}` is named nowhere else")
    return hits + [f"lint: tools-dead: tools/lint.py:0: stale ALLOW_DEAD entry `{k}`" for k in ALLOW_DEAD if k not in seen]


def ruff(root):
    if not shutil.which("ruff"):
        print("ruff: not installed, skipped")
        return []
    p = subprocess.run(
        ["ruff", "check", "--select", "F,E9", "--output-format", "concise", "tools", "renode"],
        cwd=root, capture_output=True, text=True,
    )
    return [f"lint: ruff: {l}" for l in p.stdout.splitlines() if re.match(r"\S+:\d+:\d+:", l)] if p.returncode else []


def run(root):
    return unused_pub(root) + dashes(root) + keys(root) + tools_dead(root) + ruff(root)


def selftest():
    global ALLOW, ALLOW_DEAD
    saved = ALLOW, ALLOW_DEAD
    ALLOW, ALLOW_DEAD = {}, {}
    try:
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src/sim").mkdir(parents=True)
            (root / "tools").mkdir()
            (root / "docs").mkdir()
            (root / "src/a.rs").write_text(
                "pub fn only_tests() {}\npub fn used() {}\npub struct S;\n"
                "impl S {\n    pub fn via_trait(&self) {}\n    pub fn lonely(&self) {}\n}\n"
                "impl core::fmt::Display for S {\n    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result { Ok(()) }\n}\n"
                "impl Tr for S {\n    pub fn in_impl(&self) {}\n}\n"
                '// only_tests in a comment, "{" in a string, \'}\' too\n'
                "#[cfg(test)]\npub fn gated() {}\n"
                "#[cfg(test)]\nmod tests { use super::*; fn t() { only_tests(); S.lonely(); gated(); } }\n"
            )
            (root / "src/b.rs").write_text("fn f() { crate::a::used(); }\n")
            (root / "src/sim/s.rs").write_text("pub fn sim_only() {}\nfn g() { crate::a::only_tests(); }\n")
            got = " ".join(unused_pub(root))
            for want in ("only_tests", "lonely"):
                assert f"`{want}`" in got, (want, got)
            for no in ("used", "in_impl", "gated", "sim_only", "fmt"):
                assert f"`{no}`" not in got, (no, got)
            ALLOW = {"lonely": "x", "stale": "y"}
            got = " ".join(unused_pub(root))
            assert "`lonely`" not in got and "stale ALLOW entry `stale`" in got, got
            ALLOW = {}
            (root / "docs/x.md").write_text("a \u2014 b\nfine\n")
            assert dashes(root) == ["lint: dashes: docs/x.md:1: em or en dash"]
            key = bytes(range(7, 39))
            (root / "ant_key.bin").write_bytes(key)
            (root / "src/k.rs").write_text("const K: [u8; 4] = [" + ", ".join(f"0x{b:02x}" for b in key) + "];\n")
            (root / "src/k2.rs").write_text("// " + key.hex().upper() + "\n")
            (root / "src/ok.rs").write_text("// nothing\n")
            out = keys(root)
            assert sorted(out) == ["lint: keys: src/k.rs:0: contains a licensed key",
                                   "lint: keys: src/k2.rs:0: contains a licensed key"], out
            assert not any(f.hex() in " ".join(out) for f in (key, key[:8])), "key printed"
            (root / "tools/t.py").write_text("def dead():\n    pass\n\ndef live():\n    pass\nlive()\n\ndef main():\n    pass\n")
            assert tools_dead(root) == ["lint: tools-dead: tools/t.py:1: `dead` is named nowhere else"], tools_dead(root)
            ALLOW_DEAD = {"dead": "x"}
            assert tools_dead(root) == []
    finally:
        ALLOW, ALLOW_DEAD = saved
    print("lint selftest: ok")


if __name__ == "__main__":
    if sys.argv[1:] == ["selftest"]:
        selftest()
        sys.exit(0)
    hits = run(Path(__file__).resolve().parent.parent)
    print("\n".join(hits))
    sys.exit(1 if hits else 0)
