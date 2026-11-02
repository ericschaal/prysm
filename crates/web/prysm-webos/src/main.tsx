import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { installDevService } from './dev-service';

if (import.meta.env.DEV) installDevService();

createRoot(document.getElementById('root')!).render(<StrictMode><App /></StrictMode>);
