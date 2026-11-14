import { createContext } from 'react';
import type { DiscoveryClient } from '@prysm/application-client';

/** The portal's lookup of devices to choose from; `null`: none offered. */
export const DiscoveryContext = createContext<DiscoveryClient | null>(null);
