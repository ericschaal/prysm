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
# Static frames list the four quadrants in reading order.
FRAMES = {name: (color,) * 4 for name, color in COLORS.items()}
FRAMES.update({name: (left, right, left, right) for name, (left, right) in SPLIT.items()})
peach, teal = SPLIT['coast']
amber, violet = SPLIT['dusk']
FRAMES['review-grid'] = (peach, teal, violet, amber)
SEQUENCES = {name: [frame] for name, frame in FRAMES.items()}
SEQUENCES['response'] = [FRAMES[name] for name in PATTERNS['response']]
SEQUENCES['colors'] = [FRAMES[p['clip']] for p in PATTERNS['colors']]


def generate(name, frames):
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
        for top_left, top_right, bottom_left, bottom_right in frames:
            top = top_left * (WIDTH // 2) + top_right * (WIDTH // 2)
            bottom = bottom_left * (WIDTH // 2) + bottom_right * (WIDTH // 2)
            frame = top * (HEIGHT // 2) + bottom * (HEIGHT // 2)
            for _ in range(FPS * SECONDS):
                process.stdin.write(frame)
        process.stdin.close()
        if process.wait() != 0:
            raise RuntimeError(f'encoding {name} failed')


def fields(size, first, second):
    if first == second:
        return [(0, size, first)]
    # Skip the seam where 4:2:0 chroma can blend neighboring colors.
    return [(0, size // 2 - 4, first), (size // 2 + 4, size, second)]


def check(name, frames):
    path = DEST / f'{name}.mp4'
    stream = json.loads(subprocess.check_output([
        'ffprobe', '-v', 'error', '-show_streams', '-of', 'json', str(path)]))['streams'][0]
    for key, expected in dict(codec_name='h264', pix_fmt='yuv420p', width=WIDTH, height=HEIGHT,
                              color_range='tv', color_space='bt709', color_transfer='bt709',
                              color_primaries='bt709', nb_frames=str(FPS * SECONDS * len(frames))).items():
        assert stream[key] == expected, (name, key, stream[key], expected)
    for i, quadrants in enumerate(frames):
        decoded = subprocess.check_output([
            'ffmpeg', '-v', 'error', '-ss', str(i * SECONDS + 1), '-i', str(path),
            '-frames:v', '1', '-vf', 'scale=in_color_matrix=bt709:in_range=limited:out_range=full',
            '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1'])
        assert len(decoded) == WIDTH * HEIGHT * 3
        for y0, y1, pair in fields(HEIGHT, quadrants[:2], quadrants[2:]):
            for x0, x1, expected in fields(WIDTH, *pair):
                for y in range(y0, y1):
                    row = decoded[(y * WIDTH + x0) * 3:(y * WIDTH + x1) * 3]
                    for channel, value in enumerate(expected):
                        pixels = row[channel::3]
                        assert min(pixels) >= value - 3 and max(pixels) <= value + 3, (name, i, x0, y, channel)


def stream_fragments():
    # Inline the half-second fragments so the packaged file-origin app needs no network request.
    fragments = {}
    for name in FRAMES:
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
    for name, frames in SEQUENCES.items():
        if not args.check:
            generate(name, frames)
        check(name, frames)
    bundle = DEST.parents[1] / 'src' / 'pattern-fragments.json'
    if args.check:
        fragments = json.loads(bundle.read_text())
        assert fragments.keys() == FRAMES.keys()
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
