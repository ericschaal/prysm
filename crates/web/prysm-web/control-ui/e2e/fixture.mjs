// Local simulation only: no capture, WLED, filesystem persistence, or outbound requests.
export function calibrationFixture() {
  const settings = {
    revision: '1',
    missing_required: [],
    settings: {
      source: 'prysm',
      input: {
        type: 'webos',
        capture_resolution: [640, 360],
        refresh_rate_hz: 30,
      },
      output: {
        type: 'wled',
        url: 'http://example.invalid/',
        leds: { top: 30, right: 18, bottom: 30, left: 18 },
      },
      processing: {
        brightness_percent: 70,
        shadow_lift_percent: 0,
        smoothing_seconds: 0.1,
        sample_density: 128,
        spatial_reach_percent: 10,
        remove_black_bars: true,
      },
      calibration: {
        active: 'saved',
        profiles: [
          {
            id: 'saved',
            name: 'Movie night',
            correction: { warmth: 12, tint: -8, midtones: 0 },
          },
        ],
      },
    },
  };
  const freshProfile = {
    id: 'local-profile',
    name: 'Calibration 2',
    correction: { warmth: 0, tint: 0, midtones: 0 },
  };
  let view = {
    token: 'local-phone',
    pairing: null,
    connected: true,
    saved: false,
    startup_pending: false,
    draft: freshProfile,
    step: 'choose',
    shade: 0,
    paused: false,
    compare: false,
    sequence: 0,
    ended: null,
  };
  let operation = { kind: 'idle' };
  let failSave = false;
  return {
    get view() {
      return view;
    },
    get settings() {
      return settings;
    },
    failSave(value) {
      failSave = value;
    },
    respond(path, method, body = {}) {
      if (path === '/api/settings') return { json: settings };
      if (path === '/api/state') {
        if (method === 'PUT') {
          view = { ...view, compare: false, ended: 'Lighting stopped.' };
          operation = { kind: 'succeeded', id: 'local-stop' };
          return { json: { operation_id: operation.id } };
        }
        return {
          json: {
            config_revision: '1',
            applied_revision: '1',
            source: settings.settings.source,
            calibration_active: !view.ended,
            observed: {
              power: !view.ended,
              realtime: !view.ended,
              live_override: 0,
              pipeline_phase: view.ended ? 'idle' : 'running',
            },
            operation,
            pipeline: {
              phase: view.ended ? 'idle' : 'running',
              frames_processed: 1,
            },
            wled_url: 'http://example.invalid/',
          },
        };
      }
      if (path !== '/api/calibration')
        return {
          status: 404,
          json: { message: 'Not available in the local simulation.' },
        };
      if (body.action === 'select') {
        const draft =
          body.profile == null
            ? freshProfile
            : settings.settings.calibration.profiles.find(
                (profile) => profile.id === body.profile,
              );
        if (!draft)
          return {
            status: 409,
            json: { message: 'Profile no longer exists.' },
          };
        view = {
          ...view,
          step: 'response',
          sequence: view.sequence + 1,
          draft,
        };
      }
      if (body.action === 'update' && body.sequence > view.sequence)
        view = {
          ...view,
          step: body.step,
          shade: body.shade,
          paused: body.paused,
          compare: body.compare,
          sequence: body.sequence,
          draft: { ...view.draft, correction: body.correction },
        };
      if (body.action === 'save') {
        if (failSave)
          return {
            status: 500,
            json: { message: 'Could not save the profile. Try again.' },
          };
        view = {
          ...view,
          saved: true,
          compare: false,
          ended: 'Calibration saved and active.',
          draft: { ...view.draft, name: body.name },
        };
        const calibration = settings.settings.calibration;
        calibration.profiles = [
          ...calibration.profiles.filter((p) => p.id !== view.draft.id),
          structuredClone(view.draft),
        ];
        calibration.active = view.draft.id;
      }
      if (body.action === 'cancel')
        view = { ...view, compare: false, ended: 'Calibration canceled.' };
      return { json: view };
    },
  };
}
