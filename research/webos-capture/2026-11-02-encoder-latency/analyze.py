"""Offline evidence extraction; never loads vendor code or connects to the TV."""
import hashlib
import json
from pathlib import Path
import re
import statistics
import struct
import subprocess

PROJECT = Path(__file__).resolve().parents[3]
SOURCE = PROJECT / 'work/webos-encoder/evidence/collection.ciJyC0'
OUT = Path(__file__).resolve().parent / 'evidence'
OUT.mkdir(exist_ok=True)
hashes = {}


def read(path):
    data = path.read_bytes()
    hashes[str(path.relative_to(PROJECT))] = hashlib.sha256(data).hexdigest()
    return data


ranges = {
    'libgstrvenc.so': [(0x219c, 0x221e), (0x2334, 0x2344),
                     (0x2630, 0x263a), (0x3052, 0x307a),
                     (0x3214, 0x3250), (0x3670, 0x36c0),
                     (0x3750, 0x379e), (0x3806, 0x383e),
                     (0x3fe4, 0x409c)],
    'libkadaptor.so': [(0x75b24, 0x75bae), (0x75c6e, 0x75d14),
                      (0x75db2, 0x75e2a), (0x76342, 0x763a4),
                      (0x76442, 0x76508), (0x76572, 0x76588),
                      (0x77afe, 0x77bda), (0x77d86, 0x77e08),
                      (0x110dbc, 0x110de2)],
}
binaries = {}
for name, spans in ranges.items():
    binary = SOURCE / 'elf' / name
    data = read(binary)
    assert data[:6] == b'\x7fELF\x01\x01'
    assert struct.unpack_from('<H', data, 18)[0] == 40
    binaries[name] = data
    metadata = SOURCE / 'metadata'
    sections = json.loads(read(metadata / f'{name}.sections.json'))
    # These retained libraries map the selected text virtual addresses directly.
    section = next(s for s in sections if s['name'] == '.text')
    assert section['vaddr'] == section['paddr']
    for suffix in ['imports.json', 'exports.json', 'build-id.txt']:
        content = read(metadata / f'{name}.{suffix}')
        if suffix == 'exports.json' and name == 'libkadaptor.so':
            selected = [e for e in json.loads(content) if re.search(
                r'VENC_EXT|rhal_venc|rhal_ring_|osal_Mutex|osal_Thread', e['name'])]
            content = (json.dumps(selected, indent=2) + '\n').encode()
        (OUT / f'{name}.{suffix}').write_bytes(content)
    plt = json.loads(read(metadata / f'{name}.correct-plt.json'))
    commands = ['f- sym.imp.*', 'f- rsym.*', 'e asm.bits=16', 'e asm.lines=false']
    commands += [f'pD {end-start} @ {start}' for start, end in spans]
    result = subprocess.run(
        ['/opt/homebrew/bin/r2', '-q', '-e', 'scr.color=0', '-c',
         ';'.join(commands + ['q']), str(binary)],
        capture_output=True, text=True, check=True)
    # Original tool labels can be wrong for mixed ARM/Thumb PLT stubs.
    lines = []
    for line in result.stdout.splitlines():
        call = re.search(r'\b(?:blx|bl|b\.w)\s+(0x[0-9a-f]+)', line)
        if call and call[1] in plt:
            line += ' ; relocation-verified: ' + plt[call[1]]
        lines.append(line)
    text = '\n'.join(lines) + '\n'
    expected = 'gst_video_encoder_finish_frame' if name == 'libgstrvenc.so' else 'usleep'
    assert 'relocation-verified: ' + expected in text
    (OUT / f'{name}.txt').write_text(text)

rvenc = binaries['libgstrvenc.so']
kadp = binaries['libkadaptor.so']
assert rvenc[0x36b8:0x36bc].hex() == '4ff4fa60'  # mov.w r0, 2000
assert kadp[0x75de4:0x75de6].hex() == '6420'  # movs r0, 100
assert kadp[0x76572:0x76576].hex() == '4ff47a70'  # mov.w r0, 1000
assert rvenc[0x3814:0x3816].hex() == '1928'  # cmp r0, 25
assert rvenc[0x381a:0x381c].hex() == '0023'  # enum 0 for >25
assert kadp[0x77da6:0x77daa].hex() == 'dfe803f0'  # tbb [pc, r3]
table = kadp[0x77daa:0x77daf]
assert table == bytes([3, 26, 20, 24, 22])
targets = [0x77daa + 2 * value for value in table]
fps = []
for target in targets:
    imm, opcode = kadp[target:target + 2]
    assert opcode == 0x23  # movs r3, immediate
    fps.append(imm)
assert fps == [30, 25, 24, 15, 10]
assert kadp[0x77dda:0x77ddc].hex() == '0f23'  # enum 5 also 15

log = read(PROJECT / 'work/webos-encoder/evidence/pitch-idle-probe.log').decode()
events = []
pending = None
for line in log.splitlines():
    stamp = re.match(r'(\d+):(\d+):(\d+\.\d+)', line)
    if not stamp:
        continue
    seconds = int(stamp[1])*3600 + int(stamp[2])*60 + float(stamp[3])
    if 'gst_rv_enc_handle_frame:' in line and 'plane 1 ' in line:
        assert pending is None, 'overlapping inputs: FIFO timing needs reanalysis'
        pending = seconds
    if 'gst_rv_enc_encode_cb:' in line and 'datalen' in line:
        assert pending is not None
        events.append((seconds - pending)*1000)
        pending = None
assert pending is None and len(events) == 15

bench = read(PROJECT / 'work/webos-hardware-dma/evidence/1-current.stdout')
metrics = [json.loads(line) for line in bench.decode().splitlines()]
assert len(metrics) == 3 and all(m['encoded'] == 40 for m in metrics)
for relative in ['crates/capture/webos-capture/src/encoder/gst.rs',
                 'crates/capture/webos-capture/src/session.rs']:
    read(PROJECT / relative)
summary = {
    'vendor_fps_by_enum': dict(enumerate(fps + [15])),
    'rvenc_empty_queue_sleep_us': 2000,
    'kadaptor_empty_ring_sleep_us': 100,
    'kadaptor_submit_ring_wait_sleep_us': 1000,
    'historical_plane1_log_to_callback_ms': {
        'samples': events, 'mean': statistics.mean(events),
        'min': min(events), 'max': max(events),
        'meaning': 'logged submission boundary to callback; not pure silicon time'},
    'retained_current_benchmark': metrics,
}
(OUT / 'analysis.json').write_text(json.dumps(summary, indent=2) + '\n')
(OUT / 'source-sha256.json').write_text(json.dumps(hashes, indent=2) + '\n')
print(json.dumps({k:v for k,v in summary.items() if k != 'retained_current_benchmark'}, indent=2))
