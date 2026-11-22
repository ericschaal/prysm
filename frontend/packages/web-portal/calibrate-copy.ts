/** The Calibrate page's words: patterns, screens, sliders and problems. */
import { failureOf, type Pattern } from '@prysm/application-client';
import type { CorrectionControls } from '@prysm/contracts/application';
import type { Adjustable, Screen, Section } from './calibrate-wizard';
import { noCorrection, primaries, strengthEnds } from './calibration-model';

/** What the TV shows for each pattern. */
export const patternLabels: Record<Pattern, string> = {
  gray_192: 'light grey',
  red: 'red',
  yellow: 'yellow',
  green: 'green',
  cyan: 'cyan',
  blue: 'blue',
  magenta: 'magenta',
  blue_pair: 'blue, moderate left, vivid right',
  red_pair: 'red, moderate left, vivid right',
  green_pair: 'green, moderate left, vivid right',
  coast: 'peach left, teal right',
  garden: 'rose left, sage right',
  dusk: 'amber left, violet right',
  review_grid: 'a grid of colours',
};

/** Each section's name in the progress bar. */
export const sections: Record<Section, string> = {
  white: 'White',
  red: 'Red',
  green: 'Green',
  blue: 'Blue',
  checks: 'Checks',
  review: 'Review',
};
export const sectionOrder = Object.keys(sections) as Section[];

export const primaryOf = (section: Section) =>
  primaries.find((primary) => primary.name === section);

/** A screen's one question and how to answer it. */
export function wordsOf(screen: Screen): { title: string; lead: string } {
  const primary = primaryOf(screen.section);
  if (primary) {
    const colour = primary.label.toLowerCase();
    return {
      title: `Does the ${colour} on the wall match the screen?`,
      lead: 'Compare each half of the TV with the wall behind it.',
    };
  }
  switch (screen.section) {
    case 'white':
      return {
        title: 'Does the wall look as neutral as the screen?',
        lead: 'Compare only the colour, not the brightness. Small changes are enough.',
      };
    case 'checks': {
      const fixes = screen.fixes ?? [];
      // A mixed scene can be off anywhere; a secondary in its two primaries.
      if (fixes.includes('white'))
        return {
          title: 'Do both colours on the wall match the screen?',
          lead: 'Compare each side of the wall with the same side of the TV. If one looks off, adjust white or its closest colour.',
        };
      const colour = patternLabels[screen.pattern];
      return {
        title: `Does the ${colour} on the wall match the screen?`,
        lead: `${colour[0]!.toUpperCase()}${colour.slice(1)} mixes ${fixes.join(' and ')}. If it looks off on the wall, adjust one of them.`,
      };
    }
    default:
      return {
        title: 'Name your calibration',
        lead: 'Check the glow around the whole screen. When it looks right, name it and finish.',
      };
  }
}

/**
 * This screen's own controls back to 0, whether any is not, and the control
 * a failed save shows at; `null` without controls.
 */
export function resetOf(
  section: Section,
  values: CorrectionControls,
): {
  to: Partial<CorrectionControls>;
  changed: boolean;
  control: string;
} | null {
  if (section === 'white')
    return {
      to: { warmth: 0, tint: 0 },
      changed: values.warmth !== 0 || values.tint !== 0,
      control: 'warmth',
    };
  const primary = primaryOf(section);
  if (!primary) return null;
  const { hue, strength } = values[primary.name];
  return {
    to: { [primary.name]: noCorrection[primary.name] },
    changed: hue !== 0 || strength !== 0,
    control: `${primary.name}.hue`,
  };
}

/** “More orange” → “orange”: the colour a slider's end adds. */
const leaning = (end: string) => end.replace(/^More /, '').toLowerCase();

/** What to do with a slider, from what the wall shows: one line under it. */
export const hueGuidance = ([start, end]: readonly [string, string]) =>
  `Too ${leaning(end)}? Toward ${leaning(start)}. Too ${leaning(start)}? Toward ${leaning(end)}.`;
/** White's sliders, named by their ends like the colours': warmth adds red and takes blue, tint takes green. */
export const whiteEnds = {
  warmth: ['Cooler', 'Warmer'],
  tint: ['More green', 'More pink'],
} as const;
export const whiteGuidance = {
  warmth: 'Too blue? Toward warmer. Too yellow? Toward cooler.',
  tint: hueGuidance(whiteEnds.tint),
} as const;
export const strengthGuidance = `Too vivid? Toward ${leaning(strengthEnds[0])}. Too pale? Toward ${leaning(strengthEnds[1])}.`;

/** A check's link back to what it is made of. */
export const fixLabels: Record<Adjustable, string> = {
  white: 'Adjust white',
  red: 'Adjust red',
  green: 'Adjust green',
  blue: 'Adjust blue',
};

export const tvInstruction = 'Open Prysm on the TV and choose Calibrate.';
export const needsTv = `The TV needs to show the patterns with screen sync on. ${tvInstruction}`;

/** Why the TV did not show a pattern. */
export function tvProblem(error: unknown) {
  const reason = failureOf(error);
  return reason.kind === 'needs_screen_sync'
    ? needsTv
    : `The TV pattern did not change: ${reason.message}`;
}
