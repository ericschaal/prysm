import { mountBrowserPortal } from '../../browser-portal';
import { HeadlessWebPortal } from './app';

mountBrowserPortal(HeadlessWebPortal, import.meta.url);
