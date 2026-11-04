# Processing improvements

- [ ] Adapt smoothing: suppress small unstable changes, follow sustained motion faster, and release previous colors quickly on confident scene cuts. Check noise, pans, cuts, and fades.
- [ ] Separate local edge influence from the broad scene contribution in spatial mapping. Preserve linear-light averaging; compare moving highlights, mixed colors, and spatial leakage.
- [ ] Stabilize final 8-bit output with small quantization hysteresis. Suppress near-black flicker while preserving gradual fades and true black; validate on actual LEDs.


# Calibration improvements

- [x] Add a color intensity step, selective hue correction, and a mixed-color check. Preserve the saved calibration profile. Test the full flow locally first, then run a smoke test on the TV.
