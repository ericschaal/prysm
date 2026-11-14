import type {
  Processing,
  Style,
  StyleValues,
} from '@prysm/contracts/application';
import { builtInStyles } from '@prysm/contracts/constants';

export type BuiltInStyle = Exclude<Style, 'custom'>;

/**
 * Each built-in style's values, generated from the one table in Rust: the
 * portal keeps no copy, so a retuned style arrives with the next build.
 */
export const styleValues: Readonly<Record<BuiltInStyle, StyleValues>> =
  builtInStyles;

/** Each style in words, in the order offered; Custom is last. */
export const styles: Readonly<Record<Style, { label: string; about: string }>> =
  {
    natural: {
      label: 'Natural',
      about: 'Calm in quiet scenes, follows cuts at once.',
    },
    calm: { label: 'Calm', about: 'A slow, soft glow. Nothing sudden.' },
    vivid: { label: 'Vivid', about: 'Stronger colours, follows the action.' },
    game: { label: 'Game', about: 'Reacts at once, with the least delay.' },
    custom: { label: 'Custom', about: 'Your own values.' },
  };

export const styleOrder = Object.keys(styles) as readonly Style[];

/**
 * What choosing `next` saves: the style, and on a first Custom the values
 * of the style that was ticked, so Custom starts from what the lights show.
 * Kept custom values are never replaced.
 */
export function chooseStyle<
  P extends Pick<Processing, 'style' | 'custom_style'>,
>(processing: P, next: Style): P {
  const current = processing.style.kind;
  const start =
    next === 'custom' && !processing.custom_style && current !== 'custom'
      ? { custom_style: { ...styleValues[current] } }
      : {};
  return { ...processing, ...start, style: { kind: next } };
}
