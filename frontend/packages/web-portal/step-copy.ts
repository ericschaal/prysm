/**
 * One step of setting Prysm up, the same in the setup wizard and in the
 * editor Settings opens for one section: its title, what it asks, and the
 * section's fields.
 */
export const stepCopy = {
  capture: {
    title: 'Video source',
    intro: 'Choose where the picture comes from.',
    use: 'Use this video source',
    previous: 'the previous video source',
    keep: 'Keep this video source',
  },
  output: {
    title: 'Your lights',
    intro:
      'Tell Prysm where the colours go and how your LEDs run around the screen.',
    use: 'Use these lights',
    previous: 'the previous lights',
    keep: 'Keep these lights',
  },
} as const satisfies Record<
  'capture' | 'output',
  {
    title: string;
    intro: string;
    use: string;
    previous: string;
    keep: string;
  }
>;
