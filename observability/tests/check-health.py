#!/usr/bin/env python3
"""Check actual dashboard expressions against synthetic series with Prometheus' promtool."""
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
dashboard = json.loads((ROOT / 'grafana/dashboards/prysm.json').read_text())
def flatten(items):
    return [panel for item in items for panel in [item, *flatten(item.get('panels', []))]]

panels = {p['id']: p for p in flatten(dashboard['panels'])}

def query(panel):
    return panels[panel]['targets'][0]['expr'].replace('$node', '.*').replace('$__rate_interval', '30s')

def series(name, values):
    return {'series': name, 'values': values}

def sample(value, node='computer'):
    return {'labels': f'{{node="{node}"}}', 'value': value}

def check(name, inputs, panel, expected):
    return {'name': name, 'interval': '1s', 'input_series': inputs,
            'promql_expr_test': [{'expr': query(panel), 'eval_time': '1m', 'exp_samples': expected}]}

up = series('up{job="prysm-nodes",node="computer"}', '1+0x60')
offline = series('up{job="prysm-nodes",node="computer"}', '0+0x60')

def receiver(fps, last):
    return [series('prysm_capture_requested_frames_per_second{role="Prysm",node="computer"}', f'{fps}+0x60'),
            series('prysm_receiver_last_decoded_frame_timestamp_seconds{role="Prysm",node="computer"}', f'{last}+0x60')]

tests = [check(name, [up] + receiver(fps, last), 22, [sample(expected)])
         for name, fps, last, expected in [
             ('healthy stream', 30, 59.98, 1),
             ('delayed stream', 30, 59.6, 2),
             ('stalled stream', 30, 58, 3),
             ('requested but no frame received', 30, 0, 3),
             ('intentional idle with old frames', 0, 10, 0),
             ('idle before any frames', 0, 0, 0),
             ('slow requested rate is healthy', 1, 58.5, 1),
         ]]
between_scrapes = check('healthy between five-second scrapes', [up] + receiver(30, 59.98), 22, [sample(1)])
between_scrapes['promql_expr_test'][0]['eval_time'] = '1m4s'
tests.append(between_scrapes)
tests += [check('receiver down overrides cached freshness', [offline] + receiver(30, 59.98), 22, [sample(4)]),
          check('receiver down before instrumentation exists', [offline], 22, [sample(4)]),
          check('missing instrumentation cannot report healthy or idle', [up], 22, []),
          check('no telemetry cannot report healthy', [], 22, []),
          check('TV metrics disabled does not mean receiver offline', [up] + receiver(30, 59.98) +
                [series('up{job="prysm-nodes",node="tv"}', '0+0x60')], 22, [sample(1)]),
          check('cadence at requested FPS', [up] + receiver(30, 59.98) +
                [series('prysm_pipeline_frames_total{role="Prysm",boundary="Decoded",node="computer"}', '0+30x60')], 23, [sample(1)]),
          check('idle cadence is hidden', [up] + receiver(0, 59.98) +
                [series('prysm_pipeline_frames_total{role="Prysm",boundary="Decoded",node="computer"}', '0+30x60')], 23, []),
          check('absent fault counters are zero for UP exporters', [up], 26, [sample(0)]),
          check('DOWN exporters cannot show no recent faults', [offline], 26, [])]
for fps, expected in [(30, [sample(.9375, 'tv')]), (0, [])]:
    tests.append(check(f'capture budget at {fps} FPS', [
        series('up{job="prysm-nodes",node="tv"}', '1+0x60'),
        series('prysm_capture_requested_frames_per_second{role="Capturer",node="tv"}', f'{fps}+0x60'),
        series('prysm_operation_duration_seconds_sum{component="CaptureLoop",operation="Iterate",node="tv"}', '0+0.03125x60'),
        series('prysm_operation_duration_seconds_count{component="CaptureLoop",operation="Iterate",node="tv"}', '0+1x60'),
    ], 24, expected))
all_panels = flatten(dashboard['panels'])
assert len({p['id'] for p in all_panels}) == len(all_panels)
def check_layout(items):
    for i, a in enumerate(items):
        assert a['gridPos']['x'] + a['gridPos']['w'] <= 24
        for b in items[i+1:]:
            x, y = a['gridPos'], b['gridPos']
            assert not (x['x'] < y['x']+y['w'] and y['x'] < x['x']+x['w'] and
                        x['y'] < y['y']+y['h'] and y['y'] < x['y']+x['h']), (a['title'], b['title'])
        check_layout(a.get('panels', []))
check_layout(dashboard['panels'])
compose = (ROOT / 'compose.yaml').read_text()
image = next(line.split('image:', 1)[1].strip() for line in compose.splitlines() if 'image: prom/prometheus:' in line)
subprocess.run(['docker', 'run', '--rm', '-i', '--network', 'none', '--entrypoint', '/bin/sh',
                image, '-c', 'cat > /tmp/tests.json; exec promtool test rules /tmp/tests.json'],
               input=json.dumps({'rule_files': [], 'evaluation_interval': '1s', 'tests': tests}),
               text=True, check=True)
print(f'PASS {len(tests)} health scenarios and non-overlapping dashboard layout')
