#!/usr/bin/env python3
"""Generate bundled SDR BT.709 calibration video; --check decodes and verifies it."""
import argparse
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / 'crates/web/prysm-webos/public/calibration'
WIDTH, HEIGHT, FPS, SECONDS = 640, 360, 30, 2
COLORS = {
    'black': (0, 0, 0), 'gray-32': (32, 32, 32),
    'gray-64': (64, 64, 64), 'gray-128': (128, 128, 128),
    'gray-192': (192, 192, 192), 'white': (255, 255, 255),
    'red': (255, 0, 0), 'green': (0, 255, 0), 'blue': (0, 0, 255),
    'clay': (196, 118, 95), 'leaf': (120, 166, 102), 'denim': (109, 144, 189),
    'cyan': (0, 255, 255), 'magenta': (255, 0, 255), 'yellow': (255, 255, 0),
}
SEQUENCES = {name: [rgb] for name, rgb in COLORS.items()}
SEQUENCES['response'] = [COLORS[name] for name in ['gray-64', 'gray-192', 'red', 'green', 'blue']]
SEQUENCES['colors'] = [COLORS[name] for name in ['red', 'green', 'blue', 'cyan', 'magenta', 'yellow']]

# Each mixed clip holds two unseen colors on opposite edges at the same time.
SPLIT = {'coast': ((231, 173, 138), (65, 158, 165)),
         'garden': ((201, 139, 157), (133, 155, 108)),
         'dusk': ((213, 157, 74), (129, 101, 169))}
for name, (left, right) in SPLIT.items():
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


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    DEST.mkdir(parents=True, exist_ok=True)
    for name, colors in SEQUENCES.items():
        if not args.check:
            generate(name, colors)
        check(name, colors)
    print(f'Verified {len(SEQUENCES)} bundled patterns (metadata and decoded RGB, tolerance 3/255).')
