### Transitions, 30 fps

| Setting | Cut: 90 % done, median / worst LED (ms) | Bright cut: 90 % done, median / worst (ms) | One-frame flash: shown (% of the way, median) / back within 1 step (ms) |
| --- | --- | --- | --- |
| off (raw) | 0 / 0 | 0 / 0 | 100 / 33 |
| **today: 0.1 s, cuts 0 %** | 67 / 67 | 33 / 33 | 82 / 200 |
| 0.5 s, cuts 0 % | 367 / 467 | 233 / 233 | 45 / 667 |
| 0.5 s, cuts 50 % | 233 / 333 | 133 / 133 | 79 / 767 |
| 0.5 s, cuts 100 % | 0 / 0 | 0 / 0 | 100 / 33 |
| 0.5 s, cuts 0 %, ramp 0.5 s | 367 / 467 | 233 / 267 | 13 / 667 |
| 0.5 s, cuts 50 %, ramp 0.5 s | 233 / 333 | 233 / 267 | 13 / 767 |
| 0.5 s, cuts 100 %, ramp 0.5 s | 33 / 100 | 233 / 267 | 13 / 33 |
| 0.5 s, cuts 0 %, ramp 1 s | 367 / 467 | 467 / 567 | 7 / 667 |
| 0.5 s, cuts 50 %, ramp 1 s | 233 / 333 | 467 / 567 | 7 / 767 |
| 0.5 s, cuts 100 %, ramp 1 s | 67 / 200 | 467 / 567 | 7 / 33 |

### Motion, 30 fps: behind the unsmoothed LEDs (steps)

| Setting | Fade: behind, mean / max | Fade: largest step | Pan: behind, mean / max | Pan: largest step | Local motion: behind, mean / max |
| --- | --- | --- | --- | --- | --- |
| off (raw) | 0.00 / 0.00 | 0.24 | 0.00 / 0.00 | 0.47 | 0.00 / 0.00 |
| **today: 0.1 s, cuts 0 %** | 0.05 / 0.12 | 0.21 | 0.18 / 0.27 | 0.46 | 0.06 / 2.70 |
| 0.5 s, cuts 0 % | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |
| 0.5 s, cuts 50 % | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |
| 0.5 s, cuts 100 % | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |
| 0.5 s, cuts 0 %, ramp 0.5 s | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |
| 0.5 s, cuts 50 %, ramp 0.5 s | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |
| 0.5 s, cuts 100 %, ramp 0.5 s | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |
| 0.5 s, cuts 0 %, ramp 1 s | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |
| 0.5 s, cuts 50 %, ramp 1 s | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |
| 0.5 s, cuts 100 %, ramp 1 s | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |

### Grain, 30 fps: frame-to-frame LED change, mean / p95 (steps)

| Setting | Bright, fine | Bright, blotchy | Dark, blotchy |
| --- | --- | --- | --- |
| off (raw) | 0.10 / 0.23 | 0.20 / 0.46 | 0.31 / 0.69 |
| **today: 0.1 s, cuts 0 %** | 0.05 / 0.12 | 0.11 / 0.25 | 0.17 / 0.37 |
| 0.5 s, cuts 0 % | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |
| 0.5 s, cuts 50 % | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |
| 0.5 s, cuts 100 % | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |
| 0.5 s, cuts 0 %, ramp 0.5 s | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |
| 0.5 s, cuts 50 %, ramp 0.5 s | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |
| 0.5 s, cuts 100 %, ramp 0.5 s | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |
| 0.5 s, cuts 0 %, ramp 1 s | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |
| 0.5 s, cuts 50 %, ramp 1 s | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |
| 0.5 s, cuts 100 %, ramp 1 s | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |

### Cut detector, 30 fps: frames it fires on (the same for every setting)

| Sequence | Frame changes | Cuts in it | Detector fired | Largest strength |
| --- | --- | --- | --- | --- |
| Cut | 89 | 1 | 1 | 1.00 |
| Bright cut | 89 | 1 | 1 | 1.00 |
| One-frame flash | 89 | 2 | 2 | 1.00 |
| Pan | 119 | 0 | 0 | 0.00 |
| Fade | 119 | 0 | 0 | 0.00 |
| Grain, bright fine | 149 | 0 | 0 | 0.00 |
| Grain, bright blotchy | 149 | 0 | 0 | 0.00 |
| Grain, dark blotchy | 149 | 0 | 0 | 0.00 |
| Local motion | 119 | 0 | 0 | 0.00 |

### Transitions, 60 fps

| Setting | Cut: 90 % done, median / worst LED (ms) | Bright cut: 90 % done, median / worst (ms) | One-frame flash: shown (% of the way, median) / back within 1 step (ms) |
| --- | --- | --- | --- |
| off (raw) | 0 / 0 | 0 / 0 | 100 / 17 |
| **today: 0.1 s, cuts 0 %** | 67 / 83 | 50 / 50 | 66 / 167 |
| 0.5 s, cuts 0 % | 367 / 467 | 250 / 250 | 31 / 567 |
| 0.5 s, cuts 50 % | 250 / 350 | 133 / 133 | 77 / 750 |
| 0.5 s, cuts 100 % | 0 / 0 | 0 / 0 | 100 / 17 |
| 0.5 s, cuts 0 %, ramp 0.5 s | 367 / 467 | 250 / 283 | 7 / 567 |
| 0.5 s, cuts 50 %, ramp 0.5 s | 250 / 350 | 233 / 283 | 7 / 750 |
| 0.5 s, cuts 100 %, ramp 0.5 s | 33 / 100 | 233 / 283 | 7 / 17 |
| 0.5 s, cuts 0 %, ramp 1 s | 367 / 467 | 467 / 567 | 3 / 567 |
| 0.5 s, cuts 50 %, ramp 1 s | 250 / 350 | 467 / 567 | 3 / 750 |
| 0.5 s, cuts 100 %, ramp 1 s | 67 / 217 | 467 / 567 | 3 / 17 |

### Motion, 60 fps: behind the unsmoothed LEDs (steps)

| Setting | Fade: behind, mean / max | Fade: largest step | Pan: behind, mean / max | Pan: largest step | Local motion: behind, mean / max |
| --- | --- | --- | --- | --- | --- |
| off (raw) | 0.00 / 0.00 | 0.14 | 0.00 / 0.00 | 0.23 | 0.00 / 0.00 |
| **today: 0.1 s, cuts 0 %** | 0.07 / 0.16 | 0.10 | 0.24 / 0.36 | 0.23 | 0.08 / 3.35 |
| 0.5 s, cuts 0 % | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |
| 0.5 s, cuts 50 % | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |
| 0.5 s, cuts 100 % | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |
| 0.5 s, cuts 0 %, ramp 0.5 s | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |
| 0.5 s, cuts 50 %, ramp 0.5 s | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |
| 0.5 s, cuts 100 %, ramp 0.5 s | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |
| 0.5 s, cuts 0 %, ramp 1 s | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |
| 0.5 s, cuts 50 %, ramp 1 s | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |
| 0.5 s, cuts 100 %, ramp 1 s | 0.43 / 0.81 | 0.09 | 1.41 / 2.04 | 0.22 | 0.40 / 7.34 |

### Grain, 60 fps: frame-to-frame LED change, mean / p95 (steps)

| Setting | Bright, fine | Bright, blotchy | Dark, blotchy |
| --- | --- | --- | --- |
| off (raw) | 0.10 / 0.23 | 0.20 / 0.46 | 0.31 / 0.68 |
| **today: 0.1 s, cuts 0 %** | 0.03 / 0.07 | 0.06 / 0.14 | 0.10 / 0.21 |
| 0.5 s, cuts 0 % | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |
| 0.5 s, cuts 50 % | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |
| 0.5 s, cuts 100 % | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |
| 0.5 s, cuts 0 %, ramp 0.5 s | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |
| 0.5 s, cuts 50 %, ramp 0.5 s | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |
| 0.5 s, cuts 100 %, ramp 0.5 s | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |
| 0.5 s, cuts 0 %, ramp 1 s | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |
| 0.5 s, cuts 50 %, ramp 1 s | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |
| 0.5 s, cuts 100 %, ramp 1 s | 0.01 / 0.02 | 0.01 / 0.03 | 0.02 / 0.05 |

### Cut detector, 60 fps: frames it fires on (the same for every setting)

| Sequence | Frame changes | Cuts in it | Detector fired | Largest strength |
| --- | --- | --- | --- | --- |
| Cut | 179 | 1 | 1 | 1.00 |
| Bright cut | 179 | 1 | 1 | 1.00 |
| One-frame flash | 179 | 2 | 2 | 1.00 |
| Pan | 239 | 0 | 0 | 0.00 |
| Fade | 239 | 0 | 0 | 0.00 |
| Grain, bright fine | 299 | 0 | 0 | 0.00 |
| Grain, bright blotchy | 299 | 0 | 0 | 0.00 |
| Grain, dark blotchy | 299 | 0 | 0 | 0.00 |
| Local motion | 239 | 0 | 0 | 0.00 |

