import type { SavedPreferences } from '../preferences';
import type { Link } from '../service';
import { linkCopy, type Copy } from './copy';
import { fullStatus } from './full';
import type { PortalFacts } from './situation';
import { slimButton, slimStatus } from './slim';

export function cardCopy(
  link: Link,
  draft: SavedPreferences,
  portal: PortalFacts | null,
): Copy {
  switch (link.status) {
    case 'connecting':
    case 'missing':
      return linkCopy[link.status];
    case 'live':
    case 'lost': {
      const { state } = link;
      if (state.flavor === 'full') return fullStatus(state, portal);
      return { ...slimStatus(state, portal), button: slimButton(state, draft) };
    }
  }
}
