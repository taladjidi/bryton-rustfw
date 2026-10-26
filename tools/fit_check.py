#!/usr/bin/env python3
"""Decodes the FIT files src/storage/fit.rs writes with fitdecode (an independent decoder) and checks them: CRCs,
no warnings, and the sample ride's values (the synthetic ride of fit.rs's tests). Usage (src/storage/fit.rs, module doc):
    fit_check.py target/fit/sample.fit target/fit/cut.fit [target/fit/sample_full.fit]
The third, the full copy (developer fields, recorder.rs "Two copies"), is the same ride with Alpha1, RMSSD and
Artifact in each record and the strap's beats in hrv messages: checked with both decoders, Garmin's SDK naming the
developer fields from their descriptions, the beats of both equal to the ones the test fed (count and values). The
recorder's saved pair (target/fit/recorded.fit, recorded_full.fit, from `cargo test-host two_copies_cost`):
    fit_check.py --pair target/fit/recorded.fit target/fit/recorded_full.fit
same records in both, developer fields and beats only in the full one, every beat the recorder was fed. The ride profile (fit.rs `Sample::indoor`, from
`cargo test-host fit_sample`):
    fit_check.py --profile target/fit/indoor.fit target/fit/older.fit
an indoor ride cut and resumed (sport cycling, sub_sport indoor_cycling in every lap and the session), and a ride
resumed from a record of the build before sub_sport (road).
"""
import sys
import warnings

import fitdecode

T0 = 1_161_676_800 + 631_065_600  # FIT 0 is 1989-12-31 00:00 UTC; this is 2026-10-23 08:00 UTC
DEV = ("developer_data_id", "field_description")
DEV_FIELDS = ("Alpha1", "RMSSD", "Artifact")

# The beats each test fed, ms (fit.rs tests `beats`: the sample ride's records are seconds 1 to 1200; recorder.rs
# tests `beats`: one a second through the 900 s ride, paused too)
SAMPLE_BEATS = [600 + (t * 37 + k * 11) % 400 for t in range(10, 1201) for k in range(2 + t % 2)]
RECORDED_BEATS = [250 + t for t in range(900)]


def read(path):
    msgs = []
    with warnings.catch_warnings():
        warnings.simplefilter("error")  # any decoder warning fails the check
        with fitdecode.FitReader(path, check_crc=fitdecode.CrcCheck.RAISE) as fit:
            for frame in fit:
                if isinstance(frame, fitdecode.FitDataMessage):
                    msgs.append(frame)
    return msgs


def values(m):
    return {f.name: f.value for f in m.fields if f.value is not None}


def protocol(path):
    with open(path, "rb") as f:
        return f.read(2)[1]


def garmin(path, full=False):
    """Garmin's own FIT SDK for Python (pip install garmin-fit-sdk), when installed: integrity and no errors; for
    a full copy, Alpha1 and RMSSD named by their field_description in the records."""
    try:
        from garmin_fit_sdk import Decoder, Stream
    except ImportError:
        print("  garmin-fit-sdk: not installed, skipped")
        return
    assert Decoder(Stream.from_file(path)).check_integrity()
    msgs, errors = Decoder(Stream.from_file(path)).read()
    assert errors == [], errors
    print("  garmin-fit-sdk:", {k: len(v) for k, v in msgs.items()})
    if full:
        names = {d["field_definition_number"]: d["field_name"] for d in msgs["field_description_mesgs"]}
        assert names == {0: "Alpha1", 1: "RMSSD", 2: "Artifact"}, names
        alpha = [r.get("developer_fields", {}).get(0) for r in msgs["record_mesgs"]]
        known = [a for a in alpha if a is not None and a == a]  # invalid (all ones) reads as NaN here
        print(f"  garmin-fit-sdk: Alpha1 in {len(known)} of {len(alpha)} records, e.g. {known[:3]}")
        assert all(0 < a < 3 for a in known)  # none without a strap: every value invalid


def beats(path, want):
    """The hrv messages' RR intervals, ms, with both decoders: five slots a message, the unused ones invalid, the
    rest equal to `want` (the beats fed), in order."""
    fd = [m for m in read(path) if m.name == "hrv"]
    got = []
    for m in fd:
        t = values(m)["time"]
        assert len(t) == 5, t
        got += [round(v * 1000) for v in t if v is not None]
    first = next((i for i, (a, b) in enumerate(zip(got, want)) if a != b), min(len(got), len(want)))
    assert got == want, f"fitdecode: {len(got)} beats, {len(want)} fed; the first different: {first}"
    try:
        from garmin_fit_sdk import Decoder, Stream
    except ImportError:
        print(f"  fitdecode: {len(got)} beats in {len(fd)} hrv messages, as fed (garmin-fit-sdk not installed)")
        return
    g, errors = Decoder(Stream.from_file(path)).read()
    assert errors == [], errors
    gs = [round(v * 1000) for m in g.get("hrv_mesgs", []) for v in m["time"] if v is not None]
    assert gs == want, f"garmin-fit-sdk: {len(gs)} beats, {len(want)} fed"
    print(f"  beats: {len(got)} in {len(fd)} hrv messages, as fed (fitdecode, garmin-fit-sdk)")


def read_checked(path, full):
    """The messages; the developer data only in a full copy (protocol 2.0), never in a clean one (1.0)."""
    garmin(path, full)
    msgs = read(path)
    has_dev = any(m.name in DEV for m in msgs)
    if not full:
        assert protocol(path) == 0x10 and not has_dev, "a clean copy: protocol 1.0, no developer data"
        return msgs
    assert protocol(path) == 0x20, "a full copy: protocol 2.0"
    names = [m.name for m in msgs]
    assert names[:5] == ["file_id", "developer_data_id"] + ["field_description"] * 3, names[:5]
    rec = [values(m) for m in msgs if m.name == "record"]
    with_alpha = [r for r in rec if "Alpha1" in r]
    assert all("RMSSD" in r and "Artifact" in r for r in with_alpha)
    if with_alpha:
        print(f"  fitdecode: Alpha1 in {len(with_alpha)} of {len(rec)} records, e.g. {with_alpha[0]['Alpha1']:.3f}, "
              f"RMSSD {with_alpha[0]['RMSSD']} ms, Artifact {with_alpha[0]['Artifact']} %")
    else:
        print(f"  fitdecode: no valid Alpha1 in {len(rec)} records (no strap)")
    return msgs


def check(path, finished, full=False):
    msgs = [m for m in read_checked(path, full) if m.name not in DEV and m.name != "hrv"]
    names = [m.name for m in msgs]
    records = [values(m) for m in msgs if m.name == "record"]
    print(f"{path}: {len(msgs)} messages, {len(records)} records, CRC ok")
    assert names[0] == "file_id" and names[1] == "event", names[:3]
    fid = values(msgs[0])
    print("  file_id", fid)
    assert fid["type"] == "activity" and fid["manufacturer"] == "development"
    for t in (0, 100, 205):
        r = records[t]
        print(f"  record {t}", r)
        assert r["timestamp"].timestamp() == T0 + 1 + t
        assert abs(r["position_lat"] * 180 / 2**31 - (0.5123 + 3e-5 * t)) < 1e-6
        assert abs(r["position_long"] * 180 / 2**31 - (0.4234 - 2e-5 * t)) < 1e-6
        assert abs(r["altitude"] - (35 + 0.1 * (t % 50))) < 0.2
        assert abs(r["speed"] - (7 + 0.01 * (t % 100))) < 1e-9
        assert ("heart_rate" in r) == (t >= 10) and ("power" in r) == (t >= 30 and not 200 <= t < 215)
        if full:
            assert ("Alpha1" in r) == (t >= 60)
            assert t < 60 or abs(r["Alpha1"] - (500 + t % 700) / 1000) < 1e-6
            assert ("Artifact" in r) == (t >= 60) and (t < 60 or r["Artifact"] == t % 9)
    if full and finished:
        beats(path, SAMPLE_BEATS)
    if finished:
        assert names[-4:] == ["event", "lap", "session", "activity"], names[-4:]
        for n in ("lap", "session", "activity"):
            print(f"  {n}", values(next(m for m in msgs if m.name == n)))
        s = values(next(m for m in msgs if m.name == "session"))
        dist = records[-1]["distance"]
        assert s["sport"] == "cycling" and s["total_distance"] == dist and s["max_heart_rate"] == 159
        assert s["sub_sport"] == "road", s.get("sub_sport")
        assert abs(s["total_elapsed_time"] - len(records) - 1) < 1e-9


def check_pair(clean, full):
    """The recorder's two copies of one ride: the same records, the developer fields only in the full one."""
    c = read_checked(clean, False)
    f = read_checked(full, True)
    rec = lambda ms: [{k: v for k, v in values(m).items() if k not in DEV_FIELDS}
                      for m in ms if m.name == "record"]
    rc, rf = rec(c), rec(f)
    assert not any(m.name == "hrv" for m in c), "beats in the clean copy"
    beats(full, RECORDED_BEATS)
    if rc != rf:
        # after a resume each copy restarts from its own last checkpoint: the full one can hold seconds the clean
        # one lost (or the reverse); the records both have must be the same
        common = {r["timestamp"]: r for r in rc}.keys() & {r["timestamp"]: r for r in rf}.keys()
        same = all(a == b for a, b in zip([r for r in rc if r["timestamp"] in common], [r for r in rf if r["timestamp"] in common]))
        assert same and common, "the records both copies have differ"
        print(f"{clean}, {full}: {len(rc)} and {len(rf)} records (a resume: each from its checkpoint), "
              f"the {len(common)} in both the same")
        return
    print(f"{clean}, {full}: {len(rc)} records each, the same; developer fields only in the full copy")


def check_profile(path, sub):
    """A finished ride whose laps and session say cycling, `sub` (sub_sport, its fitdecode name): with both
    decoders, Garmin's SDK giving the names its profile has (what Strava and Intervals.icu read)."""
    msgs = read(path)
    laps = [values(m) for m in msgs if m.name == "lap"]
    s = values(next(m for m in msgs if m.name == "session"))
    assert laps and all(l["sport"] == "cycling" and l["sub_sport"] == sub for l in laps), laps
    assert s["sport"] == "cycling" and s["sub_sport"] == sub, s
    try:
        from garmin_fit_sdk import Decoder, Stream
    except ImportError:
        print("  garmin-fit-sdk: not installed, skipped")
    else:
        g, errors = Decoder(Stream.from_file(path)).read()
        assert errors == [], errors
        gs = g["session_mesgs"][0]
        assert (gs["sport"], gs["sub_sport"]) == ("cycling", sub), (gs["sport"], gs["sub_sport"])
        assert all((l["sport"], l["sub_sport"]) == ("cycling", sub) for l in g["lap_mesgs"])
    print(f"{path}: {len(laps)} laps and the session: cycling, {sub} (fitdecode, garmin-fit-sdk)")


args = sys.argv[1:]
if args[:1] == ["--pair"]:
    check_pair(args[1], args[2])
elif args[:1] == ["--profile"]:
    check_profile(args[1], "indoor_cycling")
    check_profile(args[2], "road")
else:
    check(args[0], True)
    check(args[1], False)
    if len(args) > 2:
        check(args[2], True, full=True)
print("fit_check: all good")
