import { useEffect, useState } from "react";
import { Panel } from "../../design-system";
import { AccountPage } from "./account";
import { AdminPage } from "./admin";
import type { WebApiClient } from "./api";
import {
  DeviceApprovalPage,
  LoginPage,
  RecoveryPage,
  WebAuthGate,
} from "./auth";
import { OperationsPage } from "./operations";
import { ProfileDetailPage } from "./profile-detail";
import { PublicPage } from "./public";
import { matchWebRoute } from "./route-match";
import { WebShell } from "./shells";
import { WorkspacePage } from "./workspace";

export type { WebRoute } from "./route-match";
export { matchWebRoute } from "./route-match";

export function WebApp({
  api,
  initialPath,
}: {
  api: WebApiClient;
  initialPath?: string;
}) {
  const [path, setPath] = useState(
    () =>
      initialPath ??
      (typeof window === "undefined" ? "/" : window.location.pathname),
  );
  useEffect(() => {
    if (initialPath !== undefined) return;
    const update = () => setPath(window.location.pathname);
    window.addEventListener("popstate", update);
    return () => window.removeEventListener("popstate", update);
  }, [initialPath]);
  useEffect(() => {
    if (initialPath !== undefined) return;
    const navigate = (event: MouseEvent) => {
      if (
        event.defaultPrevented ||
        event.button !== 0 ||
        event.metaKey ||
        event.ctrlKey ||
        event.shiftKey ||
        event.altKey
      )
        return;
      const target = event.target;
      if (!(target instanceof Element)) return;
      const anchor = target.closest("a");
      if (
        !anchor ||
        anchor.target ||
        anchor.hasAttribute("download") ||
        anchor.origin !== window.location.origin
      )
        return;
      if (
        anchor.hash &&
        anchor.pathname === window.location.pathname &&
        anchor.search === window.location.search
      )
        return;
      event.preventDefault();
      window.history.pushState(null, "", anchor.href);
      setPath(window.location.pathname);
      window.scrollTo({ top: 0, behavior: "auto" });
    };
    document.addEventListener("click", navigate);
    return () => document.removeEventListener("click", navigate);
  }, [initialPath]);
  const route = matchWebRoute(path);
  const protectedContent = (
    <>
      {route.area === "account" ? <AccountPage api={api} path={path} /> : null}
      {route.area === "operations" ? (
        <OperationsPage api={api} path={path} />
      ) : null}
      {route.area === "workspace" ? (
        <WorkspacePage
          api={api}
          path={path}
          section={route.section}
          workspaceId={route.workspaceId}
        />
      ) : null}
      {route.area === "admin" ? <AdminPage api={api} path={path} /> : null}
      {route.area === "device" ? (
        <DeviceApprovalPage api={api} path={path} />
      ) : null}
    </>
  );
  return (
    <div>
      {route.area === "public" ? <PublicPage api={api} path={path} /> : null}
      {route.area === "login" ? <LoginPage api={api} path={path} /> : null}
      {route.area === "recovery" ? <RecoveryPage path={path} /> : null}
      {route.area === "profile" ? (
        <ProfileDetailPage api={api} profileId={route.profileId} />
      ) : null}
      {["account", "operations", "workspace", "admin", "device"].includes(
        route.area,
      ) ? (
        <WebAuthGate api={api} path={path}>
          {protectedContent}
        </WebAuthGate>
      ) : null}
      {route.area === "not-found" ? (
        <WebShell
          activePath={path}
          mode="public"
          nav={[{ href: "/", label: "Home" }]}
          title="Route error"
        >
          <Panel
            title="Page not found"
            description="Check the address or return home."
            actions={<a href="/">Return home</a>}
          />
        </WebShell>
      ) : null}
    </div>
  );
}
