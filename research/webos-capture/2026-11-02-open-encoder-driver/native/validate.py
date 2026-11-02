#!/opt/homebrew/bin/python3
"""Offline verification of retained synthetic native V4L2 clips."""
import hashlib
import json
import argparse
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--evidence", type=Path, default=ROOT / "attempt2/evidence")
parser.add_argument("--output", type=Path, default=ROOT / "validation")
args = parser.parse_args()
EVIDENCE = args.evidence
OUT = args.output
FFMPEG = "/opt/homebrew/bin/ffmpeg"
FFPROBE = "/opt/homebrew/bin/ffprobe"
OUT.mkdir(exist_ok=True)
results = {}
for count in (1, 15):
    name = f"frames-{count}"
    source = EVIDENCE / f"{name}.h264"
    events = [json.loads(x) for x in (EVIDENCE / f"{name}.jsonl").read_text().splitlines()]
    submit = [x for x in events if x["event"] == "submit"]
    captured = [x for x in events if x["event"] == "dequeue" and x["type"] == 9]
    returned = [x for x in events if x["event"] == "dequeue" and x["type"] == 10]
    summaries = [x for x in events if x["event"] == "summary"]
    assert summaries[-1]["result"] == 0
    assert len(submit) == len(captured) == len(returned) == count
    assert len({x["pts_us"] for x in submit}) == count
    by_pts = {x["pts_us"]: x for x in submit}
    assert {x["pts_us"] for x in captured} == set(by_pts)
    assert {x["pts_us"] for x in returned} == set(by_pts)
    assert all(x["result"] == 0 for x in events if x["event"] in ("streamoff", "release"))
    probe = json.loads(subprocess.check_output([FFPROBE, "-v", "error", "-count_frames", "-show_streams", "-show_packets", "-of", "json", str(source)]))
    (OUT / f"{name}.ffprobe.json").write_text(json.dumps(probe, indent=2) + "\n")
    stream = probe["streams"][0]
    assert int(stream["nb_read_frames"]) == count
    assert (stream["width"], stream["height"]) == (640, 368)
    data = source.read_bytes()
    offset = 0
    au_dir = OUT / name
    au_dir.mkdir(exist_ok=True)
    assert len(probe["packets"]) == count
    for i, (event, packet) in enumerate(zip(captured, probe["packets"])):
        assert event["data_offset"] == 0
        size = event["bytesused"]
        assert int(packet["pos"]) == offset and int(packet["size"]) == size
        (au_dir / f"frame-{i:03}.au").write_bytes(data[offset:offset + size])
        offset += size
    assert offset == len(data)
    trace = subprocess.run([FFMPEG, "-v", "verbose", "-i", str(source), "-c", "copy", "-bsf:v", "trace_headers", "-f", "null", "-"], capture_output=True, check=True)
    (OUT / f"{name}.sps-trace.txt").write_bytes(trace.stderr)
    decoded = subprocess.check_output([FFMPEG, "-v", "error", "-i", str(source), "-pix_fmt", "yuv420p", "-fps_mode", "passthrough", "-f", "rawvideo", "-"])
    frame_size = 640 * 368 * 3 // 2
    assert len(decoded) == count * frame_size
    frame_metrics = []
    for f in range(count):
        maxima, sums, checked = [], [], []
        for plane, (width, height, base) in enumerate(((640, 360, 0), (320, 180, 640*368), (320, 180, 640*368+320*184))):
            errors = []
            for row in range(height):
                start = f*frame_size+base+row*width
                for x in range(width):
                    expected = 32 + 20*((x//80+f)%8) if plane == 0 else ([90,110,150,170] if plane == 1 else [170,150,110,90])[(x*2//160+f)%4]
                    errors.append(abs(decoded[start+x]-expected))
            maxima.append(max(errors)); sums.append(sum(errors)/len(errors)); checked.append(len(errors))
        # Entire visible planes, including stripe boundaries: no tolerant interior-only shortcut.
        assert max(maxima) <= 4, (name, f, maxima)
        frame_metrics.append({"frame": f, "max_error_yuv": maxima, "mean_error_yuv": sums, "samples_checked_yuv": checked})
    latency = [x["t_us"]-by_pts[x["pts_us"]]["t_us"] for x in captured]
    steady = latency[1:]
    intervals = [submit[i]["t_us"]-submit[i-1]["t_us"] for i in range(2,count)]
    result = {
        "decoded_frames": count, "exact_capture_packet_boundaries": True,
        "stream": stream, "input_to_capture_us": latency,
        "first_capture_from_streamon_us": captured[0]["t_us"]-next(x["t_us"] for x in events if x["event"] == "streaming"),
        "steady_latency_median_us": statistics.median(steady) if steady else None,
        "steady_latency_range_us": [min(steady),max(steady)] if steady else None,
        "steady_submission_intervals_us": intervals,
        "observed_submission_fps_excluding_first_gap": 1e6/statistics.mean(intervals) if intervals else None,
        "frame_content": frame_metrics,
        "h264_sha256": hashlib.sha256(data).hexdigest(),
    }
    if count > 1:
        result["first_capture_before_second_submit_us"] = submit[1]["t_us"]-captured[0]["t_us"]
    results[name] = result
(OUT / "validation.json").write_text(json.dumps(results, indent=2) + "\n")
print(json.dumps({name: {key: value for key, value in item.items() if key not in ("stream", "frame_content")} for name, item in results.items()}, indent=2))
