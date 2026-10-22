"use client";

import { useMemo } from "react";
import {
  createWebApiClient,
  WebApp,
  type WorkspaceScope,
} from "../../frontend/src/apps/web";

export function WebRouteHost({ initialPath }: { initialPath?: string }) {
  const api = useMemo(
    () =>
      createWebApiClient({
        baseUrl: "",
        workspaceScopes: readWorkspaceScopes(
          process.env.NEXT_PUBLIC_BUGLOGIN_WORKSPACE_SCOPES,
        ),
      }),
    [],
  );

  return <WebApp api={api} initialPath={initialPath} />;
}

function readWorkspaceScopes(value: string | undefined): WorkspaceScope[] {
  if (!value) return [];
  try {
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed) || !parsed.every(isWorkspaceScope)) {
      throw new Error("expected an array of organizationId/workspaceId pairs");
    }
    return parsed;
  } catch {
    throw new Error(
      "NEXT_PUBLIC_BUGLOGIN_WORKSPACE_SCOPES must be valid JSON workspace scopes",
    );
  }
}

function isWorkspaceScope(value: unknown): value is WorkspaceScope {
  return (
    !!value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    typeof (value as Record<string, unknown>).organizationId === "string" &&
    typeof (value as Record<string, unknown>).workspaceId === "string"
  );
}
