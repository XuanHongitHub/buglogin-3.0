export type WebRoute =
  | { area: 'public' }
  | { area: 'login' }
  | { area: 'recovery' }
  | { area: 'device' }
  | { area: 'account' }
  | { area: 'operations' }
  | { area: 'migration' }
  | { area: 'workspace'; workspaceId?: string; section?: 'overview' | 'members' | 'access' | 'proxies' | 'audit' }
  | { area: 'admin' }
  | { area: 'not-found' };

export function matchWebRoute(path: string): WebRoute {
  if (path === '/' || ['/download', '/docs', '/security', '/status'].some(prefix => path === prefix || path.startsWith(`${prefix}/`))) return { area: 'public' };
  if (path === '/login') return { area: 'login' };
  if (path === '/recover') return { area: 'recovery' };
  if (path === '/device') return { area: 'device' };
  if (path === '/account' || path === '/account/profile' || path === '/account/security') return { area: 'account' };
  if (path === '/operations') return { area: 'operations' };
  if (path === '/migration') return { area: 'migration' };
  if (path === '/workspaces') return { area: 'workspace' };
  const workspace = path.match(/^\/workspaces\/([^/]+)(?:\/(overview|members|access|resources\/proxies|audit))?$/);
  if (workspace) return { area: 'workspace', workspaceId: decodeURIComponent(workspace[1]), section: workspace[2] === 'members' ? 'members' : workspace[2] === 'access' ? 'access' : workspace[2] === 'resources/proxies' ? 'proxies' : workspace[2] === 'audit' ? 'audit' : 'overview' };
  if (['/admin', '/admin/overview', '/admin/users', '/admin/organizations', '/admin/workspaces', '/admin/incidents', '/admin/audit'].includes(path)) return { area: 'admin' };
  return { area: 'not-found' };
}
