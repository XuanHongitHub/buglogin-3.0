import { type ReactNode, useCallback, useEffect, useState } from "react";
import {
  Button,
  colors,
  Panel,
  Status,
  spacing,
  typography,
} from "../../../design-system";
import { errorMessage, type LoadState } from "../api";

export type NavItem = { href: string; label: string };
export type WebShellMode =
  | "public"
  | "auth"
  | "account"
  | "workspace"
  | "operations"
  | "admin";

export function useOnline(): boolean {
  // Keep the server and first client render identical; navigator.onLine is
  // only available in the browser and can differ from the SSR assumption.
  const [online, setOnline] = useState(true);
  useEffect(() => {
    const update = () => setOnline(navigator.onLine);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);
  return online;
}

export function useLoadState<T>(
  load: (signal: AbortSignal) => Promise<T>,
  empty: (data: T) => boolean,
  emptyMessage: string,
  enabled = true,
) {
  const [_revision, setRevision] = useState(0);
  const [state, setState] = useState<LoadState<T>>({ status: "loading" });
  const retry = useCallback(() => setRevision((value) => value + 1), []);
  useEffect(() => {
    const controller = new AbortController();
    if (!enabled) {
      setState({ status: "empty", message: emptyMessage });
      return () => controller.abort();
    }
    setState({ status: "loading" });
    load(controller.signal)
      .then((data) =>
        setState(
          empty(data)
            ? { status: "empty", message: emptyMessage }
            : { status: "ready", data },
        ),
      )
      .catch((error) => {
        if (!controller.signal.aborted)
          setState({ status: "error", message: errorMessage(error) });
      });
    return () => controller.abort();
  }, [empty, emptyMessage, enabled, load]);
  return { state, retry };
}

export function AsyncState<T>({
  state,
  retry,
  children,
}: {
  state: LoadState<T>;
  retry: () => void;
  children: (data: T) => ReactNode;
}) {
  if (state.status === "loading")
    return (
      <Panel
        aria-live="polite"
        title="Loading"
        description="Requesting current data…"
      />
    );
  if (state.status === "error")
    return (
      <Panel
        title="Unable to load"
        description={state.message}
        actions={<Button onClick={retry}>Retry</Button>}
      />
    );
  if (state.status === "empty")
    return <Panel title="Nothing here yet" description={state.message} />;
  return <>{children(state.data)}</>;
}

export function MutationButton({
  action,
  children,
  variant = "secondary",
  disabled,
}: {
  action: () => Promise<void>;
  children: ReactNode;
  variant?: "primary" | "secondary" | "danger";
  disabled?: boolean;
}) {
  const [state, setState] = useState<"idle" | "loading" | "success" | "error">(
    "idle",
  );
  const run = async () => {
    setState("loading");
    try {
      await action();
      setState("success");
    } catch {
      setState("error");
    }
  };
  return (
    <span
      style={{ alignItems: "center", display: "inline-flex", gap: spacing[2] }}
    >
      <Button
        disabled={disabled}
        loading={state === "loading"}
        onClick={run}
        variant={variant}
      >
        {children}
      </Button>
      {state === "success" ? (
        <Status label="Saved" tone="success" live />
      ) : null}
      {state === "error" ? <Status label="Failed" tone="danger" live /> : null}
    </span>
  );
}

function isCurrent(item: NavItem, activePath: string): boolean {
  return (
    item.href === activePath ||
    (item.href !== "/" && activePath.startsWith(item.href))
  );
}

function Navigation({
  nav,
  activePath,
  label,
}: {
  nav: NavItem[];
  activePath: string;
  label: string;
}) {
  return (
    <>
      <nav
        aria-label={label}
        className="buglogin-web-nav buglogin-web-nav--desktop"
      >
        {nav.map((item) => (
          <a
            key={item.href}
            aria-current={isCurrent(item, activePath) ? "page" : undefined}
            href={item.href}
            className="buglogin-web-nav-link"
          >
            {item.label}
          </a>
        ))}
      </nav>
      <details className="buglogin-web-mobile-menu">
        <summary aria-label={`Open ${label}`}>Menu</summary>
        <nav aria-label={`${label} mobile`}>
          {nav.map((item) => (
            <a
              key={item.href}
              aria-current={isCurrent(item, activePath) ? "page" : undefined}
              href={item.href}
              className="buglogin-web-nav-link"
            >
              {item.label}
            </a>
          ))}
        </nav>
      </details>
    </>
  );
}

export function WebShell({
  title,
  nav,
  activePath,
  mode = "workspace",
  children,
}: {
  title: string;
  nav: NavItem[];
  activePath: string;
  mode?: WebShellMode;
  children: ReactNode;
}) {
  const online = useOnline();
  const publicMode = mode === "public";
  const authMode = mode === "auth";
  const shellClass = `buglogin-web buglogin-web--${mode}`;

  return (
    <div className={shellClass}>
      {!online ? (
        <div className="buglogin-web-offline" role="status">
          Offline{" "}
          <span>
            Live data may be stale. Remote actions are unavailable until the
            connection returns.
          </span>
        </div>
      ) : null}
      {publicMode ? (
        <>
          <header className="buglogin-web-public-header">
            <a
              className="buglogin-web-brand"
              href="/"
              aria-label="BugLogin home"
            >
              <img
                src="/buglogin-logo.png"
                alt=""
                style={{ width: 22, height: 22, objectFit: "contain" }}
              />
              <span>BugLogin</span>
            </a>
            <Navigation activePath={activePath} label="Main" nav={nav} />
            <a className="buglogin-web-header-action" href="/login">
              Đăng nhập
            </a>
          </header>
          <main className="buglogin-web-public-main" id="main-content">
            {children}
          </main>
          <footer className="buglogin-web-public-footer">
            <a
              className="buglogin-web-brand"
              href="/"
              aria-label="BugLogin home"
            >
              <img
                src="/buglogin-logo.png"
                alt=""
                style={{ width: 20, height: 20, objectFit: "contain" }}
              />
              <span>BugLogin</span>
            </a>
            <nav aria-label="Footer">
              <a href="/security">Bảo mật E2EE</a>
              <a href="/docs">Tài liệu API</a>
              <a href="/download">Tải về</a>
            </nav>
          </footer>
        </>
      ) : authMode ? (
        <>
          <header className="buglogin-web-auth-header">
            <a
              className="buglogin-web-brand"
              href="/"
              aria-label="BugLogin home"
            >
              BugLogin
            </a>
            <strong className="buglogin-web-auth-title">{title}</strong>
            <a href="/">Back to product</a>
          </header>
          <main className="buglogin-web-auth-main" id="main-content">
            {children}
          </main>
        </>
      ) : (
        <div className="buglogin-web-app-layout">
          <aside className="buglogin-web-sidebar">
            <div className="buglogin-web-sidebar-context">
              {mode === "admin"
                ? "Platform"
                : mode === "account"
                  ? "Personal"
                  : mode === "operations"
                    ? "Operations"
                    : "Workspace"}
            </div>
            <nav
              aria-label={`${title} navigation`}
              className="buglogin-web-side-nav"
            >
              {nav.map((item) => (
                <a
                  key={item.href}
                  aria-current={
                    isCurrent(item, activePath) ? "page" : undefined
                  }
                  href={item.href}
                  className="buglogin-web-nav-link"
                >
                  {item.label}
                </a>
              ))}
            </nav>
          </aside>
          <div className="buglogin-web-app-content">
            <header className="buglogin-web-app-header">
              <a
                className="buglogin-web-brand"
                href="/"
                aria-label="BugLogin home"
              >
                BugLogin
              </a>
              <div className="buglogin-web-app-context">
                <span>
                  {mode === "admin"
                    ? "Platform"
                    : mode === "account"
                      ? "Personal"
                      : mode === "operations"
                        ? "Operations"
                        : "Workspace"}
                </span>
                <strong>{title}</strong>
              </div>
              <Navigation
                activePath={activePath}
                label={`${title} navigation`}
                nav={nav}
              />
              <a className="buglogin-web-header-action" href="/account/profile">
                Account
              </a>
            </header>
            <main className="buglogin-web-app-main" id="main-content">
              {children}
            </main>
          </div>
        </div>
      )}
    </div>
  );
}

export const pageHeaderStyle = {
  color: colors.text.primary,
  margin: `0 0 ${spacing[5]}px`,
  ...typography.heading,
};
