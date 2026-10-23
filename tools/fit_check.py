#!/usr/bin/env python3
"""Decodes the FIT files src/storage/fit.rs writes with fitdecode (an independent decoder) and checks them: CRCs,
no warnings, and the sample ride's values (the synthetic ride of fit.rs's tests). Usage (src/storage/fit.rs, module doc):
    fit_check.py target/fit/sample.fit target/fit/cut.fit
"""
import sys
import warnings

import fitdecode

T0 = 1_161_676_800 + 631_065_600  # FIT 0 is 1989-12-31 00:00 UTC; this is 2026-10-23 08:00 UTC


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


def garmin(path):
    """Garmin's own FIT SDK for Python (pip install garmin-fit-sdk), when installed: integrity and no errors."""
    try:
        from garmin_fit_sdk import Decoder, Stream
    except ImportError:
        print("  garmin-fit-sdk: not installed, skipped")
        return
    assert Decoder(Stream.from_file(path)).check_integrity()
    msgs, errors = Decoder(Stream.from_file(path)).read()
    assert errors == [], errors
    print("  garmin-fit-sdk:", {k: len(v) for k, v in msgs.items()})


def check(path, finished):
    garmin(path)
    msgs = read(path)
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
        assert abs(r["position_lat"] * 180 / 2**31 - (48.8566 + 3e-5 * t)) < 1e-6
        assert abs(r["position_long"] * 180 / 2**31 - (2.3522 - 2e-5 * t)) < 1e-6
        assert abs(r["altitude"] - (35 + 0.1 * (t % 50))) < 0.2
        assert abs(r["speed"] - (7 + 0.01 * (t % 100))) < 1e-9
        assert ("heart_rate" in r) == (t >= 10) and ("power" in r) == (t >= 30 and not 200 <= t < 215)
    if finished:
        assert names[-4:] == ["event", "lap", "session", "activity"], names[-4:]
        for n in ("lap", "session", "activity"):
            print(f"  {n}", values(next(m for m in msgs if m.name == n)))
        s = values(next(m for m in msgs if m.name == "session"))
        dist = records[-1]["distance"]
        assert s["sport"] == "cycling" and s["total_distance"] == dist and s["max_heart_rate"] == 159
        assert abs(s["total_elapsed_time"] - len(records) - 1) < 1e-9


check(sys.argv[1], True)
check(sys.argv[2], False)
print("fit_check: all good")
