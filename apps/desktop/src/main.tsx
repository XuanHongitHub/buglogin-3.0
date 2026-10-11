import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { createTauriDesktopApi } from './bridge.js';
import { AuthBootstrap } from './AuthBootstrap.js';
import { ProfileWorkspace } from './ProfileWorkspace.js';

const root = document.getElementById('root');
if (!root) throw new Error('Desktop root element not found');

const api = createTauriDesktopApi();
createRoot(root).render(
  <StrictMode>
    <AuthBootstrap api={api}><ProfileWorkspace api={api} /></AuthBootstrap>
  </StrictMode>,
);
