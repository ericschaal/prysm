#!/usr/bin/env python3
"""Generate bundled SDR BT.709 calibration video; --check decodes and verifies it."""
import argparse
import base64
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / 'crates/web/prysm-webos/public/calibration'
WIDTH, HEIGHT, FPS, SECONDS = 640, 360, 30, 2
PATTERNS = json.loads((ROOT / 'crates/web/calibration-patterns.json').read_text())


def rgb(hex_color):
    return bytes.fromhex(hex_color.removeprefix('#'))


COLORS = {p['clip']: rgb(p.get('reference', p['swatch']))
          for group in ('grays', 'colors', 'muted') for p in PATTERNS[group]}
SPLIT = {p['clip']: (rgb(p['leftColor']), rgb(p['rightColor'])) for p in PATTERNS['pairs']}
SEQUENCES = {name: [rgb] for name, rgb in COLORS.items()}
SEQUENCES['response'] = [COLORS[name] for name in PATTERNS['response']]
SEQUENCES['colors'] = [COLORS[p['clip']] for p in PATTERNS['colors']]
for name, (left, _) in SPLIT.items():
    SEQUENCES[name] = [left]


def generate(name, colors):
    command = ['ffmpeg', '-v', 'error', '-y', '-f', 'rawvideo', '-pixel_format', 'rgb24',
               '-video_size', f'{WIDTH}x{HEIGHT}', '-framerate', str(FPS), '-i', 'pipe:0',
               '-vf', 'scale=in_range=full:out_range=limited:out_color_matrix=bt709,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709',
               '-an', '-c:v', 'libx264', '-profile:v', 'baseline', '-level:v', '3.0',
               '-preset', 'medium', '-crf', '12', '-threads', '1', '-pix_fmt', 'yuv420p',
               '-g', str(FPS), '-keyint_min', str(FPS), '-sc_threshold', '0',
               '-color_range', 'tv', '-colorspace', 'bt709', '-color_primaries', 'bt709',
               '-color_trc', 'bt709', '-movflags', '+faststart', '-map_metadata', '-1',
               str(DEST / f'{name}.mp4')]
    with subprocess.Popen(command, stdin=subprocess.PIPE) as process:
        for rgb in colors:
            left, right = SPLIT.get(name, (rgb, rgb))
            row = bytes(left) * (WIDTH // 2) + bytes(right) * (WIDTH // 2)
            frame = row * HEIGHT
            for _ in range(FPS * SECONDS):
                process.stdin.write(frame)
        process.stdin.close()
        if process.wait() != 0:
            raise RuntimeError(f'encoding {name} failed')


def check(name, colors):
    path = DEST / f'{name}.mp4'
    stream = json.loads(subprocess.check_output([
        'ffprobe', '-v', 'error', '-show_streams', '-of', 'json', str(path)]))['streams'][0]
    for key, expected in dict(codec_name='h264', pix_fmt='yuv420p', width=WIDTH, height=HEIGHT,
                              color_range='tv', color_space='bt709', color_transfer='bt709',
                              color_primaries='bt709', nb_frames=str(FPS * SECONDS * len(colors))).items():
        assert stream[key] == expected, (name, key, stream[key], expected)
    for i, rgb in enumerate(colors):
        decoded = subprocess.check_output([
            'ffmpeg', '-v', 'error', '-ss', str(i * SECONDS + 1), '-i', str(path),
            '-frames:v', '1', '-vf', 'scale=in_color_matrix=bt709:in_range=limited:out_range=full',
            '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1'])
        assert len(decoded) == WIDTH * HEIGHT * 3
        # Check the sampled edges and both fields, excluding the 4:2:0 seam.
        bands = [(0, WIDTH, rgb)] if name not in SPLIT else [
            (0, WIDTH // 2 - 4, SPLIT[name][0]), (WIDTH // 2 + 4, WIDTH, SPLIT[name][1])]
        for start, end, expected in bands:
            for channel in range(3):
                for y in range(HEIGHT):
                    row = decoded[(y*WIDTH+start)*3+channel:(y*WIDTH+end)*3+channel:3]
                    assert max(abs(min(row)-expected[channel]), abs(max(row)-expected[channel])) <= 3, (name, i, channel)


def stream_fragments():
    # Inline the half-second fragments so the packaged file-origin app needs no network request.
    fragments = {}
    for name in (*COLORS, *SPLIT):
        data = subprocess.check_output([
            'ffmpeg', '-v', 'error', '-i', str(DEST / f'{name}.mp4'), '-frames:v', str(FPS // 2),
            '-c:v', 'copy', '-movflags', 'empty_moov+default_base_moof+frag_keyframe+skip_trailer',
            '-f', 'mp4', 'pipe:1'])
        fragments[name] = base64.b64encode(data).decode()
    return json.dumps(fragments, indent=2) + '\n'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    DEST.mkdir(parents=True, exist_ok=True)
    for name, colors in SEQUENCES.items():
        if not args.check:
            generate(name, colors)
        check(name, colors)
    bundle = DEST.parents[1] / 'src' / 'pattern-fragments.json'
    if args.check:
        fragments = json.loads(bundle.read_text())
        assert fragments.keys() == (COLORS | SPLIT).keys()
        # Compare encoded picture packets, independent of the FFmpeg muxer version.
        probe = ['ffprobe', '-v', 'error', '-select_streams', 'v:0', '-show_entries',
                 'packet=data_hash:stream=duration,avg_frame_rate', '-show_data_hash', 'sha256',
                 '-of', 'json']
        for name, fragment in fragments.items():
            bundled = json.loads(subprocess.check_output([*probe, 'pipe:0'],
                                                         input=base64.b64decode(fragment)))
            stream = bundled['streams'][0]
            assert float(stream['duration']) == 0.5, (name, stream)
            assert stream['avg_frame_rate'] == f'{FPS}/1', (name, stream)
            original = json.loads(subprocess.check_output([*probe, str(DEST / f'{name}.mp4')]))
            packets = bundled['packets']
            assert len(packets) == FPS // 2 and packets == original['packets'][:FPS // 2], name
    else:
        bundle.write_text(stream_fragments())
    print(f'Verified {len(SEQUENCES)} bundled patterns (metadata and decoded RGB, tolerance 3/255).')
