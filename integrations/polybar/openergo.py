#!/usr/bin/env -S python3 -u

import subprocess
import time
from dataclasses import dataclass

RETRY_SECONDS = 1.0

RED = "#e06c75"
FG = "#abb2bf"


@dataclass
class Metric:
    icon: str
    pct: int = -1


# nerd Font glyphs
METRICS: dict[str, Metric] = {
    "DAY": Metric(icon="\uf185"),   # nf-fa-sun_o
    "BREAK": Metric(icon="\uf0f4"), # nf-fa-coffee
    "REST": Metric(icon="\uf252"),  # nf-fa-hourglass_half
    "PAIN": Metric(icon="\uef89"),  # nf-fa-notes_medical
}

ORDER = ("REST", "BREAK", "DAY", "PAIN")


def segment(name: str) -> str:
    m = METRICS[name]
    if m.pct < 0:
        return f"%{{F{FG}}}{m.icon}%{{F-}} ??"
    if m.pct >= 100:
        return f"%{{F{RED}}}{m.icon} XX%%{{F-}}"
    return f"%{{F{FG}}}{m.icon}%{{F-}} {m.pct}%"


def emit() -> None:
    print("  ".join(segment(n) for n in ORDER), flush=True)


def reset() -> None:
    for m in METRICS.values():
        m.pct = -1


def run_once() -> None:
    """Spawn the CLI and process its output until it exits or fails."""
    try:
        proc = subprocess.Popen(
            ["openergo"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
    except OSError:
        return
    assert proc.stdout is not None
    for raw in proc.stdout:
        parts = raw.split()
        if len(parts) != 3:
            continue
        if parts[0] == "PAIN":
            # PAIN <label> <ratio>; ignore label, show as a single percentage.
            try:
                ratio = float(parts[2])
            except ValueError:
                continue
            pct = max(0, min(100, int(ratio * 100)))
            m = METRICS["PAIN"]
            if pct != m.pct:
                m.pct = pct
                emit()
            continue
        name, cur_s, lim_s = parts
        m = METRICS.get(name)
        if m is None:
            continue
        try:
            cur = float(cur_s)
            lim = float(lim_s)
        except ValueError:
            continue
        if lim <= 0:
            continue
        pct = min(100, int(cur * 100 / lim))
        if pct != m.pct:
            m.pct = pct
            emit()
    proc.wait()


def main() -> None:
    while True:
        reset()
        emit()
        run_once()
        time.sleep(RETRY_SECONDS)



if __name__ == "__main__":
    try:
        main()
    except (KeyboardInterrupt, BrokenPipeError):
        pass
