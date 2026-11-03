import { useId } from 'react';
import type { SettingsValues } from './api';
import { ledLimits } from './settings-draft';

type LedCounts = NonNullable<SettingsValues['output']>['leds'];

export function LedLayout({
  leds,
}: {
  leds: { [K in keyof LedCounts]: string | number };
}) {
  const id = useId();
  const counts = [leds.top, leds.right, leds.bottom, leds.left].map((value) => {
    const count = Number(value);
    return String(value).trim() &&
      Number.isInteger(count) &&
      count >= ledLimits.min &&
      count <= ledLimits.max
      ? count
      : null;
  });
  const [top, right, bottom, left] = counts;
  const total = counts.every((count) => count !== null)
    ? counts.reduce((sum, count) => sum + (count ?? 0), 0)
    : null;
  return (
    <figure className="led-layout">
      <svg viewBox="0 0 360 224" role="img" aria-labelledby={`${id}-title`}>
        <title id={`${id}-title`}>
          LED layout: {top ?? 'unset'} top, {right ?? 'unset'} right,{' '}
          {bottom ?? 'unset'} bottom, {left ?? 'unset'} left
        </title>
        <defs>
          <linearGradient id={`${id}-led-color`} x2="1" y2="1">
            <stop stopColor="#c080b4" />
            <stop offset=".5" stopColor="#9b86cf" />
            <stop offset="1" stopColor="#7cbbc9" />
          </linearGradient>
          <linearGradient id={`${id}-screen-glass`} x2=".9" y2="1">
            <stop stopColor="#3d3b50" />
            <stop offset="1" stopColor="#272b3a" />
          </linearGradient>
        </defs>
        <rect
          className="screen-body"
          fill={`url(#${id}-screen-glass)`}
          x="78"
          y="54"
          width="204"
          height="116"
          rx="5"
        />
        <path
          className="led-edge"
          stroke={`url(#${id}-led-color)`}
          d="M90 45h180 M291 66v92 M270 179H90 M69 158V66"
        />
        <path className="screen-stand" d="M180 171v20m-28 0h56" />
        <text x="180" y="27" textAnchor="middle">
          {top ?? '—'}
        </text>
        <text x="325" y="116" textAnchor="middle">
          {right ?? '—'}
        </text>
        <text x="180" y="216" textAnchor="middle">
          {bottom ?? '—'}
        </text>
        <text x="35" y="116" textAnchor="middle">
          {left ?? '—'}
        </text>
        <text className="screen-label" x="180" y="106" textAnchor="middle">
          {total ?? '—'} LEDs
        </text>
        <text className="screen-sublabel" x="180" y="128" textAnchor="middle">
          viewed from the front
        </text>
      </svg>
    </figure>
  );
}
