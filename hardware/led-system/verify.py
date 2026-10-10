#!/usr/bin/env python3
"""Run fresh KiCad checks and inspect actual schematic/PCB nets, then check exports."""
import csv
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET
import zipfile
import pcbnew as pcb

ROOT = Path(__file__).resolve().parent
CLI = Path('/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli')
BOARDS = ('gpio-interface', 'power-distribution', 'corner-interface')

def run(*args):
    result = subprocess.run([str(CLI), *map(str, args)], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError('KiCad failed: ' + ' '.join(map(str, args)) + '\n' +
                           result.stdout + result.stderr[-3000:])
    return result.stdout

def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def check(root=ROOT, publish=False):
    actual = {}
    with tempfile.TemporaryDirectory(prefix='prysm-cad-check-') as scratch:
        for name in BOARDS:
            folder = root / name
            out = folder if publish else Path(scratch) / name
            out.mkdir(exist_ok=True)
            sch = folder / (name + '.kicad_sch')
            boardfile = folder / (name + '.kicad_pcb')
            run('sch', 'erc', '--format', 'json', '--severity-all', '--exit-code-violations',
                '-o', out / 'erc.json', sch)
            run('pcb', 'drc', '--format', 'json', '--severity-all', '--schematic-parity',
                '--refill-zones', '--exit-code-violations', '-o', out / 'drc.json', boardfile)
            drc = json.loads((out / 'drc.json').read_text())
            assert not any(drc[k] for k in ('violations', 'unconnected_items', 'schematic_parity')), name
            erc = json.loads((out / 'erc.json').read_text())
            assert not any(sheet['violations'] for sheet in erc['sheets']), name
            netfile = Path(scratch) / (name + '.xml')
            run('sch', 'export', 'netlist', '--format', 'kicadxml', '-o', netfile, sch)
            tree = ET.parse(netfile)
            schematic = {(n.attrib['ref'], int(n.attrib['pin'])): net.attrib['name']
                         for net in tree.findall('./nets/net') for n in net.findall('node')
                         if not n.attrib['ref'].startswith('PWR') and
                         not net.attrib['name'].startswith('unconnected-')}
            b = pcb.LoadBoard(str(boardfile))
            footprints = {f.GetReference(): f for f in b.GetFootprints()}
            assembly = {'gpio-interface': 'GPIO PCB', 'power-distribution': 'Distribution PCB',
                        'corner-interface': 'Corner PCB'}[name]
            with (root / 'bom.csv').open() as f:
                bomrefs = {ref for row in csv.DictReader(f) if row['Assembly'] == assembly
                           for ref in row['References'].split()}
            assert bomrefs == {r for r in footprints if not r.startswith('H')}, name + ': BOM coverage'
            pads = {(f.GetReference(), int(p.GetNumber())): p.GetNetname()
                    for f in b.GetFootprints() for p in f.Pads()
                    if p.GetNumber().isdigit() and p.GetNetCode() and
                    not p.GetNetname().startswith('unconnected-')}
            with (folder / 'connections.csv').open() as f:
                csvnets = {(r['Reference'], int(r['Pin'])): r['Net'] for r in csv.DictReader(f)}
            assert pads == schematic == csvnets, name + ': actual CAD and pin map disagree'
            actual[name] = pads
            copper = '0.07' if name == 'power-distribution' else '0.035'
            assert boardfile.read_text().count('(thickness ' + copper + ')') == 2, name + ': copper stackup'
            assert not any(t.GetWidth(pcb.F_Cu) < pcb.FromMM(.7) for t in b.GetTracks()
                           if isinstance(t, pcb.PCB_VIA)), name + ': small via'
            if name != 'power-distribution':
                u = footprints['U1']
                assert u.GetFPID().GetLibNickname() == 'Package_SO'
                assert u.GetFPID().GetLibItemName() == 'SOIC-8_3.9x4.9mm_P1.27mm'
                assert u.GetValue() == 'ISO7720FD'
                assert len(list(u.Pads())) == 8
                ground1, ground2 = pads['U1', 4], pads['U1', 5]
                supply1, supply2 = pads['U1', 1], pads['U1', 8]
                assert ground1 != ground2 and supply1 != supply2
                for ref in ('C1', 'C2'):
                    expected = (supply1, ground1) if ref == 'C1' else (supply2, ground2)
                    assert (pads[ref, 1], pads[ref, 2]) == expected
                assert pads['R1', 2] == pads['R2', 2] == ground1
                assert pads['U1', 2] == pads['R1', 1]
                assert pads['U1', 3] == pads['R2', 1]
                assert pads['U1', 7] == pads['R3', 1]
                assert pads['U1', 6] == pads['R4', 1]
                # Only U1 may span both local electrical domains.
                left = {supply1, ground1, pads['U1', 2], pads['U1', 3]}
                for f in b.GetFootprints():
                    nets = {p.GetNetname() for p in f.Pads() if p.GetNetCode() and
                            not p.GetNetname().startswith('unconnected-')}
                    assert f.GetReference() == 'U1' or not (nets & left and nets - left), name + ': domain bridge'
            if name == 'gpio-interface':
                j = footprints['J1']
                assert j.GetLayer() == pcb.B_Cu and j.GetOrientationDegrees() == -90
                assert {p for r, p in pads if r == 'J1'} == {1, 6, 19, 23}
                pin1 = next(p for p in j.Pads() if p.GetNumber() == '1')
                assert pin1.GetPosition() == pcb.VECTOR2I(pcb.FromMM(108.37), pcb.FromMM(74.77))
            print(name + ': fresh ERC/DRC/parity and actual net/footprint checks passed', flush=True)
    hat, power, corner = (actual[n] for n in BOARDS)
    assert hat['J1', 1] == hat['U1', 1] == '+3V3_PI'
    assert hat['J1', 6] == hat['U1', 4] == 'GND_PI'
    assert hat['J1', 19] == hat['U1', 2] == 'MOSI_3V3'
    assert hat['J1', 23] == hat['U1', 3] == 'SCLK_3V3'
    assert hat['J3', 1] == hat['U1', 8] == '+5V_A'
    assert hat['J3', 4] == hat['U1', 5] == 'GND_A'
    assert hat['J3', 2] == hat['R3', 2] and hat['J3', 3] == hat['R4', 2]
    assert power['J1', 1] == 'GND' and power['J1', 2] == '+5V_IN'
    for fuse, connector, net in [('F2', 'J2', '+5V_A'), ('F3', 'J3', '+5V_B')]:
        assert power[fuse, 1] == '+5V_BUS'
        assert power[fuse, 2] == power[connector, 1] == net
    assert not any(ref in ('F4', 'J4') for ref, pin in power)
    assert corner['J1', 1] == corner['U1', 1] == '+5V_A'
    assert corner['J1', 4] == corner['U1', 4] == 'GND_A'
    assert corner['J1', 2] == corner['U1', 2] and corner['J1', 3] == corner['U1', 3]
    assert corner['J2', 1] == corner['U1', 8] == '+5V_B'
    assert corner['J2', 4] == corner['U1', 5] == 'GND_B'
    assert corner['J2', 2] == corner['R3', 2] and corner['J2', 3] == corner['R4', 2]
    assert round(168 * .06 * .75, 2) == 7.56
    print('Local power domains and open branch topology passed. Physical harness is not validated.', flush=True)

def check_package(root=ROOT):
    manifest = json.loads((root / 'build-manifest.json').read_text())
    for relative, digest in manifest['sha256'].items():
        assert sha256(root / relative) == digest, 'Stale delivery: ' + relative
    with zipfile.ZipFile(root / 'prysm-led-system-rev-c.zip') as z:
        expected = set(manifest['sha256']) | {'build-manifest.json'}
        assert set(z.namelist()) == expected, 'Delivery ZIP file list differs'
        for relative in expected:
            assert z.read(relative) == (root / relative).read_bytes(), 'Stale ZIP: ' + relative
    for name in BOARDS:
        folder = root / name
        files = list((folder / 'fabrication').iterdir())
        suffixes = {'F_Cu.gtl', 'B_Cu.gbl', 'F_Mask.gts', 'B_Mask.gbs',
                    'F_Silkscreen.gto', 'B_Silkscreen.gbo', 'Edge_Cuts.gm1',
                    'PTH.drl', 'NPTH.drl', 'job.gbrjob'}
        assert {p.name for p in files} == {name + '-' + s for s in suffixes}, 'Incomplete fabrication: ' + name
        with zipfile.ZipFile(folder / 'fabrication.zip') as z:
            assert set(z.namelist()) == {p.name for p in files}
            for p in files:
                assert z.read(p.name) == p.read_bytes(), 'Stale fabrication ZIP: ' + name
    print('Manifest and all delivery/fabrication ZIPs match the current files.', flush=True)

if __name__ == '__main__':
    check()
    check_package()
