#!/usr/bin/env python3
"""Checks reviewers did by hand, run by check.sh (python3 -I tools/lint.py; `selftest` proves each fires).

Each hit prints `lint: <check>: file:line: what`; the exit status is 1 on any hit.
  unused-pub  a pub / pub(crate) item of src/ that only tests (or doc tests) use: make it #[cfg(test)] or ALLOW it
  dashes      U+2014 / U+2013 in a tracked text file (AGENTS.md, Wording)
  keys        a licensed key file's bytes (raw, hex in any layout, decimal / 0x byte arrays, a little-endian
              literal, base64) inside a tracked or new file, or a key file itself tracked; only file names are
              printed. The key files are looked for in the worktree and in the main checkout.
  tools-dead  a function of tools/ or renode/ that nothing names
  ruff        pyflakes errors (F, E9) of tools/ and renode/, when ruff is installed

Limit of unused-pub: it matches names, not paths, so a common name (`new`, `len`) used anywhere in src/ counts as
used and an unused pub item of that name is not found.
"""
import base64
import contextlib
import io
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
        # tracked plus untracked not ignored (as check.sh's docs guard), so a new file is scanned before git add
        cmd = ["git", "-C", str(root), "ls-files", "-z", "-co", "--exclude-standard"]
        names = {n for n in subprocess.run(cmd, capture_output=True, check=True).stdout.decode().split("\0") if n}
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


def key_search(k):
    """-> a function data -> bool: does `data` carry key `k` in one of the forms a person or a tool writes."""
    rk = k[::-1]
    hexes = (k.hex(), rk.hex())  # the reverse is a little-endian u64 literal
    b64 = []
    for off in range(3):
        s = base64.b64encode(bytes(off) + k).decode().rstrip("=")
        b64.append(s[(0, 2, 3)[off] : -1].encode())  # drop the chars the prefix and the padding share

    def hit(data):
        if k in data or any(b in data for b in b64):
            return True
        if len(data) > 2_000_000:  # shortcut: no text forms in a huge file, raise if one is ever needed
            return False
        text = data.decode("latin-1").lower()
        flat = re.sub(r"[^0-9a-f]", "", text.replace("0x", "").replace("\\x", ""))
        if any(h in flat for h in hexes):
            return True
        toks = (int(t, 16) if t[1:2] == "x" else int(t) for t in re.findall(r"0x[0-9a-f]+|\d+", text))
        nums = bytes(v for v in toks if v < 256)
        return k in nums or rk in nums

    return hit


def main_checkout(root):
    cmd = ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"]
    p = subprocess.run(cmd, cwd=root, capture_output=True, text=True)
    return Path(p.stdout.strip()).parent if p.returncode == 0 and p.stdout.strip() else root


def keys(root, homes=None):
    homes = homes or [root, main_checkout(root)]
    found = {}
    for h in homes:
        for k in KEY_FILES:
            f = h / k
            if f.is_file() and f.stat().st_size >= 8 and k not in found:
                found[k] = f
    if not found:
        print("keys: no key files, skipped")
        return []
    print("keys: checked " + " ".join(sorted(found)))
    searches = [key_search(f.read_bytes()) for f in found.values()]
    hits = []
    for n in files(root):
        if Path(n).name in KEY_FILES:
            hits.append(f"lint: keys: {n}:0: a licensed key file is tracked")
        elif any(s((root / n).read_bytes()) for s in searches):
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
    dirs = [d for d in ("tools", "renode") if (root / d).is_dir()]
    cmd = ["ruff", "check", "--select", "F,E9", "--output-format", "concise", *dirs]
    p = subprocess.run(cmd, cwd=root, capture_output=True, text=True)
    if not p.returncode:
        return []
    found = [f"lint: ruff: {l}" for l in p.stdout.splitlines() if re.match(r"\S+:\d+:\d+:", l)]
    return found or [f"lint: ruff: exit {p.returncode}: {p.stderr[:200]}"]


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
            (root / "tools/t.py").write_text("def dead():\n    pass\n\ndef live():\n    pass\nlive()\n\ndef main():\n    pass\n")
            assert tools_dead(root) == ["lint: tools-dead: tools/t.py:1: `dead` is named nowhere else"], tools_dead(root)
            ALLOW_DEAD = {"dead": "x"}
            assert tools_dead(root) == []
    finally:
        ALLOW, ALLOW_DEAD = saved
    selftest_keys()
    print("lint selftest: ok")


def selftest_keys():
    key = bytes([0xA1, 0x3C, 0x5E, 0x77, 0x09, 0xD2, 0xB4, 0x68])
    pairs = [f"{b:02x}" for b in key]
    forms = {
        "raw": b"\0" + key,
        "hex": ("// " + key.hex().upper()).encode(),
        "hex sep": " ".join(pairs).encode(),
        "decimal": ("[" + ", ".join(map(str, key)) + "]").encode(),
        "rustfmt": ("const K: [u8; 8] = [\n" + "".join(f"    0x{p},\n" for p in pairs) + "];\n").encode(),
        "bytes repr": ("b'" + "".join(f"\\x{p}" for p in pairs) + "'").encode(),
        "le u64": ("0x" + "_".join(key[::-1].hex()[i : i + 4] for i in range(0, 16, 4))).encode(),
    }
    for pre in range(3):
        for post in range(3):
            forms[f"b64 {pre}/{post}"] = base64.b64encode(b"\x11\x22"[:pre] + key + b"ab"[:post]) + b"\n"
    leak = [key.hex(), key[::-1].hex()]
    with tempfile.TemporaryDirectory() as d, tempfile.TemporaryDirectory() as h:
        root, home = Path(d), Path(h)
        (home / "ant_key.bin").write_bytes(key)
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        (root / "src").mkdir()
        (root / "src/ok.rs").write_text("// 0123456789abcdef fedcba9876543210 [1, 2, 3, 4, 5, 6, 7, 8] [161, 60, 94, 119, 9, 210, 180]\n")
        (root / "src/part.rs").write_text("[0xa1, 0x3c, 0x5e, 0x77, 0x09, 0xd2, 0xb4]\n")
        with contextlib.redirect_stdout(io.StringIO()) as out:
            assert keys(root, [home]) == []  # near misses and unrelated numbers are no hit
            for name, data in forms.items():
                (root / "src/new.rs").write_bytes(data)  # untracked, never added: still scanned
                got = keys(root, [home])
                assert got == ["lint: keys: src/new.rs:0: contains a licensed key"], (name, got)
                assert not any(x in " ".join(got) for x in leak), name
            (root / "src/new.rs").unlink()
            (root / "ant_key.bin").write_bytes(b"x")
            subprocess.run(["git", "-C", str(root), "add", "ant_key.bin"], check=True)
            assert keys(root, [home]) == ["lint: keys: ant_key.bin:0: a licensed key file is tracked"]
            assert keys(root, [root / "nowhere"]) == []
        text = out.getvalue()
        assert "no key files, skipped" in text and "checked ant_key.bin" in text, text
        assert not any(x in text for x in leak), "key printed"
    if shutil.which("ruff"):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "tools").mkdir()
            (Path(d) / "tools/bad.py").write_text("import os\n")
            got = ruff(Path(d))
            assert len(got) == 1 and "tools/bad.py:1:" in got[0], got


if __name__ == "__main__":
    if sys.argv[1:] == ["selftest"]:
        selftest()
        sys.exit(0)
    hits = run(Path(__file__).resolve().parent.parent)
    print("\n".join(hits))
    sys.exit(1 if hits else 0)
