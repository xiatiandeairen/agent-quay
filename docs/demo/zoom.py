#!/usr/bin/env python3
"""Adds camera moves to the raw demo recording: docs/demo/zoom.py <raw.mp4> <out dir>.

Writes demo.mp4 (1920x1080) and demo.gif (1120x630, 15 fps, 128 colors to stay near 6 MB) to <out dir>. Only ffmpeg is needed.
SHOTS and CUTS are in seconds of the raw recording; if you change demo.tape, re-time them
by looking at the raw video (for example one frame per second:
ffmpeg -i raw.mp4 -vf "fps=1,scale=720:-1,tile=4x4" sheet%d.png).
"""
import math
import subprocess
import sys
import tempfile

# Raw frame is 2880x1620: window (80,80)-(2800,1540), title bar 60 px, panes split at x=1440, y=852.
FULL = (1440, 810, 1.0)
LEFT = (846, 575, 1.85)       # agent A over agent B: both builds and their results
C_TOP = (2100, 525, 2.0)      # agent C typing the tests
DASH = (2100, 1215, 2.0)      # aq status --top: running, queued, waiting for a heavy slot
TOP_ROW = (1440, 640, 1.4)    # agents A and C borrowing simulators

# (time, camera): the camera holds a shot until the next entry, then eases to it.
SHOTS = [
    (0.0, FULL), (0.7, FULL),
    (1.5, LEFT), (4.3, LEFT),      # A starts the build, B starts the same build
    (5.0, C_TOP), (6.1, C_TOP),    # C starts the tests
    (6.9, DASH), (9.3, DASH),      # the tests wait for the heavy slot, then start
    (10.1, LEFT), (12.6, LEFT),    # A's build is ok, B gets the same result with reused:true
    (13.6, FULL), (22.4, FULL),    # the tests finish
    (23.2, TOP_ROW), (25.6, TOP_ROW),  # A and C each borrow a fresh simulator
    (26.6, FULL),
]
# Idle stretches cut from the output: (start, end).
CUTS = [(17.0, 22.4)]
FPS = 25


def ease(a, b, t0, t1, log=False):
    """ffmpeg expression for a smooth move from a to b between t0 and t1 (variable T)."""
    s = f"(((T-{t0})/{t1 - t0}))"
    s = f"({s}*{s}*(3-2*{s}))"
    if log:
        return f"exp({math.log(a)}+({math.log(b) - math.log(a)})*{s})"
    return f"({a}+({b - a})*{s})"


def track(i, log=False):
    """Piecewise expression over SHOTS for camera component i."""
    expr = str(SHOTS[-1][1][i])
    for (t0, c0), (t1, c1) in reversed(list(zip(SHOTS, SHOTS[1:]))):
        seg = str(c0[i]) if c0[i] == c1[i] else ease(c0[i], c1[i], t0, t1, log)
        expr = f"if(lt(T,{t1}),{seg},{expr})"
    return expr.replace("T", f"(in/{FPS})")


def main():
    raw, out = sys.argv[1], sys.argv[2]
    keep = " * ".join(f"not(between(t,{a},{b}))" for a, b in CUTS) or "1"
    graph = (
        # zoompan crops at whole input pixels; at 2x the steps are too small to see as jitter.
        f"[0:v]fps={FPS},scale=iw*2:ih*2:flags=lanczos,"
        f"zoompan=z='{track(2, log=True)}'"
        f":x='clip(2*{track(0)}-iw/zoom/2,0,iw-iw/zoom)'"
        f":y='clip(2*{track(1)}-ih/zoom/2,0,ih-ih/zoom)'"
        f":d=1:fps={FPS}:s=1920x1080,"
        f"select='{keep}',setpts=N/{FPS}/TB[v]"
    )
    with tempfile.NamedTemporaryFile("w", suffix=".txt") as script:
        script.write(graph)
        script.flush()
        subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-y", "-i", raw, "-/filter_complex", script.name,
             "-map", "[v]", "-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p",
             "-movflags", "+faststart", f"{out}/demo.mp4"],
            check=True,
        )
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-i", f"{out}/demo.mp4", "-filter_complex",
         "fps=15,scale=1120:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];"
         "[b][p]paletteuse=dither=none:diff_mode=rectangle",
         f"{out}/demo.gif"],
        check=True,
    )


if __name__ == "__main__":
    main()
