"""The portal's size-and-rate presets resolved against `Modes`, as
frontend/packages/web-portal/capture-modes.ts `resolve` does."""

PRESETS = {
    'balanced': {'width': 640, 'height': 360, 'fps': 30},
    'smooth': {'width': 640, 'height': 360, 'fps': 60},
    'efficient': {'width': 320, 'height': 180, 'fps': 30},
}


def _contains(span, value):
    return span['min'] <= value <= span['max'] and (value - span['min']) % span['step'] == 0


def _at_least(span, target):
    if target <= span['min']:
        return span['min']
    if target > span['max']:
        return None
    steps = -(-(target - span['min']) // span['step'])
    return span['min'] + steps * span['step']


def delivers(modes, size):
    return any(_contains(m['width'], size['width']) and _contains(m['height'], size['height'])
               and _contains(m['fps'], size['fps']) for m in modes)


def resolve(modes, target):
    """The target where a mode takes it, else the smallest covering size (16:9 first) at its rate; else None."""
    if delivers(modes, target):
        return dict(target)
    covering = []
    for mode in modes:
        width = _at_least(mode['width'], target['width'])
        height = _at_least(mode['height'], target['height'])
        if width is not None and height is not None and _contains(mode['fps'], target['fps']):
            covering.append({'width': width, 'height': height, 'fps': target['fps']})
    wide = [size for size in covering if size['width'] * 9 == size['height'] * 16]
    pool = wide or covering
    return min(pool, key=lambda size: size['width'] * size['height'], default=None)


def describe(modes):
    """One line per mode, `640x360 @ 30` or spans `176-1920/2 x ... @ 1-30`."""
    def span(s):
        return str(s['min']) if s['min'] == s['max'] else f"{s['min']}-{s['max']}/{s['step']}"
    return [f"{span(m['width'])} x {span(m['height'])} @ {span(m['fps'])}" for m in modes]
