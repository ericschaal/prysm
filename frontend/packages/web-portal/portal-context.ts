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
  type ReactNode,
  type RefObject,
  type SetStateAction,
} from 'react';
import { useLocation, useNavigate } from 'react-router';
import type { AdapterSection, AdapterUis } from './adapters';
import type { DeletedProfile } from './profiles';
import { pathOfRoute } from './routes';
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
}

type Section = AdapterSection;
export type Host = WebPortalProps<Section, Section>;

/** Where the portal goes: to a page, or back to where it came from. */
export interface Nav {
  /** Open `route` (a `#/…` route) as a new page, at its top. */
  go: (route: string) => void;
  /**
   * Return to `route`: back through the history when the page was opened
   * from there, so it comes back at its place; else open it.
   */
  back: (route: string) => void;
  /** The page this one was opened from, if it was opened in the portal. */
  from: string | null;
}

export function useNav(): Nav {
  const navigate = useNavigate();
  const location = useLocation();
  const from = (location.state as { from?: string } | null)?.from ?? null;
  return {
    go: (route) =>
      void navigate(pathOfRoute(route), {
        state: { from: location.pathname },
      }),
    back: (route) =>
      void (from === pathOfRoute(route)
        ? navigate(-1)
        : navigate(pathOfRoute(route))),
    from,
  };
}

/** What every page of a connected portal shares. */
export interface Portal {
  host: Host;
  snapshot: ApplicationSnapshot<Section, Section>;
  online: boolean;
  commands: ApplicationRuntime<Section, Section>['commands'];
  dispatch: ReturnType<typeof useApplicationDispatch<Section, Section>>;
  nav: Nav;
  path: PathState;
  /** The live problem of one side, in plain words; `null` while it works. */
  problemAt: (section: 'capture' | 'output') => string | null;
  saveProcessing: (expected: number, processing: Processing) => Promise<number>;
  tryProcessing: (processing: Processing) => Promise<unknown>;
  /** "Lost the connection", once that lasts; `null` otherwise. */
  offline: ReactNode;
  /** How setup ended, shown on Home and Settings until setup opens again. */
  setupNotice: string;
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

/** The page chrome: the header's connection line and Settings button. */
export interface Chrome {
  connectionLabel: string;
  outOfTouch: boolean;
  /** Home's Settings button, while Home shows the set-up portal. */
  settingsLink: (() => void) | null;
  developer: boolean;
}

/** The elements the frame reaches into: kept apart from what renders. */
export interface PortalRefs {
  /** The setup notice, focused when it appears. */
  notice: RefObject<HTMLParagraphElement | null>;
  /** Home's grid, which takes the live LED colours. */
  homeGrid: RefObject<HTMLDivElement | null>;
}

export const PortalContext = createContext<{
  portal: Portal | null;
  chrome: Chrome;
} | null>(null);
export const RefsContext = createContext<PortalRefs | null>(null);

function usePortalContext() {
  const context = use(PortalContext);
  if (!context) throw new Error('Outside the portal.');
  return context;
}

export const useChrome = () => usePortalContext().chrome;

/** What every page shares; only under a connected portal. */
export function usePortal(): Portal {
  const { portal } = usePortalContext();
  if (!portal) throw new Error('Not connected yet.');
  return portal;
}

export function usePortalRefs(): PortalRefs {
  const refs = use(RefsContext);
  if (!refs) throw new Error('Outside the portal.');
  return refs;
}
