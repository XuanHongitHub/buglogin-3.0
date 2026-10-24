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
  ["/profileId=prf_web_1", "profile"],
  ["/missing", "not-found"],
] as const;

for (const [path, area] of cases) {
  if (matchWebRoute(path).area !== area)
    throw new Error(`Route mismatch: ${path}`);
}

const profileRoute = matchWebRoute("/profileId=prf_web_1");
if (profileRoute.area !== "profile" || profileRoute.profileId !== "prf_web_1")
  throw new Error("Profile detail URL did not resolve its profile ID");
