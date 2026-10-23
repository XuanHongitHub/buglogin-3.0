import { matchWebRoute } from "./route-match.ts";

const cases = [
  ["/", "public"],
  ["/account/security", "account"],
  ["/workspaces", "workspace"],
  ["/workspaces/workspace-1/access", "workspace"],
  ["/workspaces/workspace-1/audit", "workspace"],
  ["/admin/users", "admin"],
  ["/admin/incidents", "admin"],
  ["/admin/audit", "admin"],
  ["/missing", "not-found"],
] as const;

for (const [path, area] of cases) {
  if (matchWebRoute(path).area !== area)
    throw new Error(`Route mismatch: ${path}`);
}
