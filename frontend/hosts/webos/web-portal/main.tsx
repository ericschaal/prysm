import { mountBrowserPortal } from '../../browser-portal';
import { WebosWebPortal } from './app';

mountBrowserPortal(WebosWebPortal, import.meta.url);
