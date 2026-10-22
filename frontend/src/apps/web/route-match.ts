export type WebRoute =
  | { area: "public" }
  | { area: "login" }
  | { area: "recovery" }
  | { area: "device" }
  | { area: "account" }
  | { area: "operations" }
  | { area: "migration" }
  | {
      area: "workspace";
      workspaceId?: string;
      section?:
        | "overview"
        | "members"
        | "access"
        | "proxies"
        | "audit"
        | "profiles"
        | "groups";
    }
  | { area: "admin" }
  | { area: "not-found" };

export function matchWebRoute(path: string): WebRoute {
  if (
    path === "/" ||
    [
      "/download",
      "/docs",
      "/security",
      "/status",
      "/pricing",
      "/features",
    ].some((prefix) => path === prefix || path.startsWith(`${prefix}/`))
  )
    return { area: "public" };
  if (path === "/login") return { area: "login" };
  if (path === "/recover") return { area: "recovery" };
  if (path === "/device") return { area: "device" };
  if (
    path === "/account" ||
    path === "/account/profile" ||
    path === "/account/security" ||
    path === "/account/sessions"
  )
    return { area: "account" };
  if (path === "/operations") return { area: "operations" };
  if (path === "/migration") return { area: "migration" };
  if (path === "/workspaces" || path === "/workspaces/")
    return { area: "workspace" };
  const workspace = path.match(
    /^\/workspaces\/([^/]+)(?:\/(overview|members|access|resources\/proxies|proxies|audit|profiles|groups))?$/,
  );
  if (workspace) {
    const sec = workspace[2];
    const section =
      sec === "members"
        ? "members"
        : sec === "access"
          ? "access"
          : sec === "resources/proxies" || sec === "proxies"
            ? "proxies"
            : sec === "audit"
              ? "audit"
              : sec === "profiles"
                ? "profiles"
                : sec === "groups"
                  ? "groups"
                  : "overview";
    return {
      area: "workspace",
      workspaceId: decodeURIComponent(workspace[1]),
      section,
    };
  }
  if (
    [
      "/admin",
      "/admin/overview",
      "/admin/users",
      "/admin/organizations",
      "/admin/workspaces",
      "/admin/incidents",
      "/admin/audit",
    ].includes(path)
  )
    return { area: "admin" };
  return { area: "not-found" };
}
