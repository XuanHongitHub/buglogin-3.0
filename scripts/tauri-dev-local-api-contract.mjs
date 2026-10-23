const REQUIRED_DESKTOP_OPERATIONS = [
  ["POST", "/v1/auth/desktop/login", "auth.desktop.login"],
  ["GET", "/v1/account", "account.get"],
  ["GET", "/v1/organizations", "organization.list"],
  ["GET", "/v1/organizations/{organizationId}/workspaces", "workspace.list"],
  ["POST", "/v1/organizations/{organizationId}/workspaces", "workspace.create"],
  ["GET", "/v1/profiles", "profile.list"],
  ["POST", "/v1/profiles", "profile.create"],
  ["GET", "/v1/workspaces/{workspaceId}/proxies", "proxy.list"],
  ["POST", "/v1/workspaces/{workspaceId}/proxies", "proxy.create"],
  ["GET", "/v1/workspaces/{workspaceId}/grants/page", "grant.list.page"],
];

export function isRootTauriDevInvocation(command, args) {
  return (
    command === "pnpm" &&
    Array.isArray(args) &&
    args[0] === "--dir" &&
    args[1] === "apps/desktop" &&
    args[2] === "exec" &&
    args[3] === "tauri" &&
    args[4] === "dev"
  );
}

export function hasDurableTauriDevPersistence(persistenceMode) {
  return persistenceMode === "postgres" || persistenceMode === "sqlite";
}

export function parseTauriDevApiBaseUrl(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error("BUGLOGIN_DEV_API_BASE_URL must be an explicit loopback HTTP URL");
  }
  const hostname = url.hostname.toLowerCase();
  if (
    url.protocol !== "http:" ||
    !["localhost", "127.0.0.1", "[::1]", "::1"].includes(hostname) ||
    !url.port ||
    url.username ||
    url.password ||
    url.pathname !== "/" ||
    url.search ||
    url.hash
  ) {
    throw new Error("BUGLOGIN_DEV_API_BASE_URL must be an explicit loopback HTTP origin");
  }
  return url.origin;
}

export function isForbiddenTauriDevCargoTargetDir(value, platform = process.platform) {
  if (platform !== "win32") return false;
  const drive = /^(?:\\\\\?\\)?([a-z]):[\\/]/i.exec(value)?.[1]?.toLowerCase();
  return drive === "c" || drive === "e" || drive === "f";
}

export function findMissingTauriDevOperations(openApi) {
  if (
    !openApi ||
    typeof openApi !== "object" ||
    !openApi.paths ||
    typeof openApi.paths !== "object"
  ) {
    return ["valid OpenAPI paths object"];
  }

  return REQUIRED_DESKTOP_OPERATIONS.flatMap(([method, path, operationId]) => {
    const operation = openApi.paths[path]?.[method.toLowerCase()];
    return operation?.operationId === operationId
      ? []
      : [`${method} ${path} (${operationId})`];
  });
}

export function validateMcpStatusResponse(status) {
  return Boolean(
    status &&
      typeof status === "object" &&
      typeof status.enabled === "boolean" &&
      status.endpoint === "/v1/mcp" &&
      status.authentication === "scoped-api-token",
  );
}
