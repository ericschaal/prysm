import type { ApplicationRuntime } from '@prysm/application-state';
import type { Processing } from '@prysm/contracts/application';
import type {
  ApplicationSnapshot,
  ColorsClient,
  DiscoveryClient,
  PreviewClient,
} from '@prysm/application-client';
import type { useApplicationDispatch } from '@prysm/application-react';
import {
  createContext,
  use,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from 'react';
import { useLocation, useNavigate } from 'react-router';
import type { AdapterSection, AdapterUis } from './adapters';
import type { DeletedProfile } from './profiles';
import { pages, pathTo, type Place } from './routes';
import type { PathState } from './setup-card';

export interface WebPortalProps<
  C extends AdapterSection,
  O extends AdapterSection,
> {
  application: ApplicationRuntime<C, O>;
  /** The capture kinds this host registers, in display order. */
  captures: AdapterUis<NoInfer<C>>;
  /** The output kinds this host registers, in display order. */
  outputs: AdapterUis<NoInfer<O>>;
  /** The live preview; without one the portal shows no preview. */
  preview?: PreviewClient;
  /**
   * The LEDs' colours, for the glow Home casts while screen sync runs;
   * without one the glow is the designed one.
   */
  colors?: ColorsClient;
  /**
   * Finds devices to choose from in the capture and lights editors; without
   * one they offer only typed fields.
   */
  discovery?: DiscoveryClient;
  /**
   * Whether Prysm now serves another build of this page, asked when the
   * connection returns (`scriptReplaced`); without one it is never asked.
   */
  replaced?: () => Promise<boolean>;
}

type Section = AdapterSection;
export type Host = WebPortalProps<Section, Section>;

/** Where the portal goes: to a page, or back to where it came from. */
export interface Nav {
  /** Open `place` as a new page, at its top. */
  go: (place: Place) => void;
  /**
   * Return to `place`: back through the history when this page was opened
   * from there, so it comes back at its place; else open it.
   */
  back: (place: Place) => void;
  /** Return to the page this one was opened from in the portal, else Home. */
  backToOpener: () => void;
}

/**
 * The path of the page this history entry was opened from, which `go` keeps
 * in its state. Any other state (from another build, or none) means none.
 */
function openedFrom(state: unknown): string | null {
  return typeof state === 'object' &&
    state !== null &&
    'from' in state &&
    typeof state.from === 'string'
    ? state.from
    : null;
}

export function useNav(): Nav {
  const navigate = useNavigate();
  const location = useLocation();
  const { pathname } = location;
  const from = openedFrom(location.state);
  return {
    go: (place) => void navigate(pathTo(place), { state: { from: pathname } }),
    back: (place) =>
      void (from === pathTo(place) ? navigate(-1) : navigate(pathTo(place))),
    backToOpener: () =>
      void (from === null ? navigate(pages.home) : navigate(-1)),
  };
}

/** What every page of a connected portal shares. */
export interface Portal {
  host: Host;
  snapshot: ApplicationSnapshot<Section, Section>;
  online: boolean;
  commands: ApplicationRuntime<Section, Section>['commands'];
  dispatch: ReturnType<typeof useApplicationDispatch<Section, Section>>;
  path: PathState;
  /** The live problem of one side, in plain words; `null` while it works. */
  problemAt: (section: 'capture' | 'output') => string | null;
  saveProcessing: (expected: number, processing: Processing) => Promise<number>;
  tryProcessing: (processing: Processing) => Promise<unknown>;
  /** How setup ended, shown on Home and Settings until setup opens again. */
  setupNotice: string;
  /** Where the setup notice shows, focused by the frame when it appears. */
  noticeRef: RefObject<HTMLParagraphElement | null>;
  openSetup: () => void;
  /**
   * What each editor saved over last, offered back until it is replaced,
   * also after leaving the editor.
   */
  previous: { capture: Section | null; output: Section | null };
  setPrevious: (previous: Portal['previous']) => void;
  /** Profiles deleted on this visit, shown in their places with Undo. */
  deletedProfiles: Record<string, DeletedProfile>;
  setDeletedProfiles: Dispatch<SetStateAction<Record<string, DeletedProfile>>>;
}

/** What every page shows, connected or not: the header's line and notices. */
export interface Chrome {
  connectionLabel: string;
  /** The connection has been missing long enough to say so. */
  outOfTouch: boolean;
  developer: boolean;
  /** Prysm was updated under this page: every page offers a reload. */
  updated: boolean;
}

/** The host's props, for the frame the router renders. */
export const HostContext = createContext<Host | null>(null);
export const ChromeContext = createContext<Chrome | null>(null);
/** Provided only once the first state has arrived. */
export const PortalContext = createContext<Portal | null>(null);

export function useHost(): Host {
  const host = use(HostContext);
  if (!host) throw new Error('Outside the portal.');
  return host;
}

export function useChrome(): Chrome {
  const chrome = use(ChromeContext);
  if (!chrome) throw new Error('Outside the portal.');
  return chrome;
}

/** What every page shares; only on the pages of a connected portal. */
export function usePortal(): Portal {
  const portal = use(PortalContext);
  if (!portal) throw new Error('Outside a connected portal.');
  return portal;
}
