import { useCallback, useEffect, useState } from "react";
import {
  Button,
  colors,
  Field,
  Panel,
  Status,
  spacing,
  typography,
} from "../../../design-system";
import {
  type AccountSummary,
  type ApiTokenSummary,
  errorMessage,
  type WebApiClient,
  WebApiError,
  type WebSessionSummary,
} from "../api";
import { pageHeaderStyle, useOnline, WebShell } from "../shells";

const accountNav = [
  { href: "/account/profile", label: "Hồ sơ tài khoản (Profile)" },
  { href: "/account/security", label: "Bảo mật & Tokens (Security)" },
  { href: "/account/sessions", label: "Phiên đăng nhập (Sessions)" },
  { href: "/workspaces", label: "Workspaces" },
];

type AccountLoadState =
  | { status: "loading" | "offline" | "forbidden" | "empty" }
  | { status: "error"; message: string; correlationId: string }
  | { status: "ready"; data: AccountSummary };

export function AccountPage({
  api,
  path,
}: {
  api: WebApiClient;
  path: string;
}) {
  const online = useOnline();
  const load = useCallback(
    (signal: AbortSignal) => api.reads.account(signal),
    [api],
  );
  const { state, retry } = useAccountState(load, online);

  return (
    <WebShell
      activePath={path}
      mode="account"
      nav={accountNav}
      title="Account Management"
    >
      <h1 style={pageHeaderStyle}>{accountTitle(path)}</h1>
      <AccountState api={api} path={path} retry={retry} state={state} />
    </WebShell>
  );
}

function accountTitle(path: string) {
  if (path === "/account/security") return "Bảo mật tài khoản & API Tokens";
  if (path === "/account/sessions") return "Quản lý phiên thiết bị hoạt động";
  return "Hồ sơ tài khoản & Định danh";
}

function AccountState({
  api,
  path,
  retry,
  state,
}: {
  api: WebApiClient;
  path: string;
  retry: () => void;
  state: AccountLoadState;
}) {
  if (state.status === "loading") {
    return (
      <Panel
        aria-live="polite"
        title="Đang tải dữ liệu tài khoản…"
        description="Đang xác thực thông tin tài khoản và quyền hạn từ BugLogin Cloud API…"
      />
    );
  }
  if (state.status === "offline") {
    return (
      <Panel
        title="Dữ liệu tài khoản không khả dụng khi ngoại tuyến"
        description="Vui lòng kết nối Internet để kiểm tra định danh và các phiên bảo mật mới nhất."
        actions={<Button onClick={retry}>Thử lại (Retry)</Button>}
      />
    );
  }
  if (state.status === "forbidden") {
    return (
      <Panel
        title="Truy cập bị từ chối (403 Forbidden)"
        description="Phiên làm việc hiện tại không có quyền truy cập vào phân vùng tài khoản này."
      />
    );
  }
  if (state.status === "error") {
    return (
      <Panel
        title="Không thể tải dữ liệu tài khoản"
        description={`${state.message} (Correlation ID: ${state.correlationId})`}
        actions={<Button onClick={retry}>Thử lại (Retry)</Button>}
      />
    );
  }
  if (state.status === "empty") {
    return (
      <Panel
        title="Không tìm thấy hồ sơ tài khoản"
        description="Dịch vụ trả về hồ sơ rỗng. Vui lòng thử đăng nhập lại."
        actions={<Button onClick={retry}>Thử lại (Retry)</Button>}
      />
    );
  }
  if (state.status !== "ready") return null;

  return <AccountRoute account={state.data} api={api} path={path} />;
}

function AccountRoute({
  account,
  api,
  path,
}: {
  account: AccountSummary;
  api: WebApiClient;
  path: string;
}) {
  const logout = async () => {
    try {
      await api.auth.logout(new AbortController().signal);
    } finally {
      navigateToLogin();
    }
  };

  const isProfile = path === "/account" || path === "/account/profile";
  const isSecurity = path === "/account/security";
  const isSessions = path === "/account/sessions";

  return (
    <div style={{ display: "grid", gap: spacing[4] }}>
      {/* Account Overview Hero Card */}
      <Panel
        actions={
          <div
            style={{ display: "flex", gap: spacing[2], alignItems: "center" }}
          >
            <Status
              label={account.status ?? "Hoạt động (Active)"}
              tone="success"
            />
            <Button onClick={logout} variant="secondary">
              Đăng xuất (Sign out)
            </Button>
          </div>
        }
        title="Định danh tài khoản (Account Identity)"
        description="Định danh duy nhất được xác thực trên nền tảng BugLogin Antidetect Cloud."
      >
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
            gap: spacing[3],
            marginTop: spacing[2],
          }}
        >
          <div
            style={{
              padding: spacing[3],
              borderRadius: 6,
              background: colors.surface.sunken,
              border: `1px solid ${colors.border.subtle}`,
            }}
          >
            <div style={{ ...typography.caption, color: colors.text.muted }}>
              Account ID
            </div>
            <code
              style={{
                fontSize: "14px",
                fontWeight: 600,
                display: "block",
                marginTop: 4,
              }}
            >
              {account.accountId}
            </code>
          </div>

          <div
            style={{
              padding: spacing[3],
              borderRadius: 6,
              background: colors.surface.sunken,
              border: `1px solid ${colors.border.subtle}`,
            }}
          >
            <div style={{ ...typography.caption, color: colors.text.muted }}>
              Trạng thái bảo mật
            </div>
            <div
              style={{
                fontSize: "14px",
                fontWeight: 600,
                marginTop: 4,
                color: colors.semantic.success.text,
              }}
            >
              ✓ E2EE Cloud Hardened
            </div>
          </div>

          <div
            style={{
              padding: spacing[3],
              borderRadius: 6,
              background: colors.surface.sunken,
              border: `1px solid ${colors.border.subtle}`,
            }}
          >
            <div style={{ ...typography.caption, color: colors.text.muted }}>
              Phiên bản dữ liệu (Revision)
            </div>
            <div style={{ fontSize: "14px", fontWeight: 600, marginTop: 4 }}>
              rev-{account.revision ?? 1}
            </div>
          </div>

          <div
            style={{
              padding: spacing[3],
              borderRadius: 6,
              background: colors.surface.sunken,
              border: `1px solid ${colors.border.subtle}`,
            }}
          >
            <div style={{ ...typography.caption, color: colors.text.muted }}>
              Ngày khởi tạo
            </div>
            <div style={{ fontSize: "14px", fontWeight: 600, marginTop: 4 }}>
              {account.createdAt
                ? new Date(account.createdAt).toLocaleDateString()
                : "Hôm nay"}
            </div>
          </div>
        </div>
      </Panel>

      {/* Render Subsections */}
      {isProfile && (
        <>
          <Panel
            title="Đặc quyền & Hạn ngạch (Quota & Capabilities)"
            description="Tài nguyên phân bổ trực tiếp cho tài khoản và các Workspace liên kết."
          >
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
                gap: spacing[3],
              }}
            >
              <div
                style={{
                  padding: spacing[3],
                  borderRadius: 6,
                  border: `1px solid ${colors.border.subtle}`,
                }}
              >
                <div
                  style={{ ...typography.caption, color: colors.text.muted }}
                >
                  Workspace Quota
                </div>
                <div
                  style={{ fontSize: "24px", fontWeight: 700, marginTop: 4 }}
                >
                  Không giới hạn
                </div>
                <div
                  style={{
                    ...typography.caption,
                    color: colors.text.muted,
                    marginTop: 2,
                  }}
                >
                  Quản lý đa tổ chức & workspace độc lập
                </div>
              </div>

              <div
                style={{
                  padding: spacing[3],
                  borderRadius: 6,
                  border: `1px solid ${colors.border.subtle}`,
                }}
              >
                <div
                  style={{ ...typography.caption, color: colors.text.muted }}
                >
                  Profile Quota
                </div>
                <div
                  style={{ fontSize: "24px", fontWeight: 700, marginTop: 4 }}
                >
                  Enterprise Pool
                </div>
                <div
                  style={{
                    ...typography.caption,
                    color: colors.text.muted,
                    marginTop: 2,
                  }}
                >
                  Tự động cấp phát theo Workspace Policy
                </div>
              </div>

              <div
                style={{
                  padding: spacing[3],
                  borderRadius: 6,
                  border: `1px solid ${colors.border.subtle}`,
                }}
              >
                <div
                  style={{ ...typography.caption, color: colors.text.muted }}
                >
                  Antidetect Engine
                </div>
                <div
                  style={{
                    fontSize: "24px",
                    fontWeight: 700,
                    marginTop: 4,
                    color: colors.brand[700],
                  }}
                >
                  BugLogin Core
                </div>
                <div
                  style={{
                    ...typography.caption,
                    color: colors.text.muted,
                    marginTop: 2,
                  }}
                >
                  Chống phát hiện Canvas/WebGL/WebRTC
                </div>
              </div>
            </div>
          </Panel>

          <SessionInventory api={api} compact />
        </>
      )}

      {isSecurity && (
        <>
          <Panel
            title="Chính sách bảo mật Zero-Trust"
            description="Cấu hình bảo vệ phiên truy cập, mã hóa đầu cuối và các quy tắc đăng nhập."
          >
            <div style={{ display: "grid", gap: spacing[3] }}>
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  padding: `${spacing[3]}px 0`,
                  borderBottom: `1px solid ${colors.border.subtle}`,
                }}
              >
                <div>
                  <strong style={typography.bodySmall}>
                    Mã hóa E2EE Cloud S3 Vault
                  </strong>
                  <div
                    style={{ ...typography.caption, color: colors.text.muted }}
                  >
                    Dữ liệu cookies, local storage và thông số proxy được mã hóa
                    khóa riêng trước khi đồng bộ.
                  </div>
                </div>
                <Status label="Bắt buộc (Active)" tone="success" />
              </div>

              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  padding: `${spacing[3]}px 0`,
                  borderBottom: `1px solid ${colors.border.subtle}`,
                }}
              >
                <div>
                  <strong style={typography.bodySmall}>
                    Xác thực thiết bị phần cứng (Hardware Fingerprint Lock)
                  </strong>
                  <div
                    style={{ ...typography.caption, color: colors.text.muted }}
                  >
                    Mỗi phiên làm việc Desktop gắn liền với UUID máy trạm và chữ
                    ký mật mã TPM/Secure Enclave.
                  </div>
                </div>
                <Status label="Bật (Enabled)" tone="success" />
              </div>

              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  padding: `${spacing[3]}px 0`,
                }}
              >
                <div>
                  <strong style={typography.bodySmall}>
                    Thời hạn phiên làm việc (Session Keepalive)
                  </strong>
                  <div
                    style={{ ...typography.caption, color: colors.text.muted }}
                  >
                    Giữ phiên làm việc liên tục khi thao tác thường xuyên; chỉ
                    hủy phiên khi đăng xuất hoặc quản trị viên thu hồi.
                  </div>
                </div>
                <Status label="Không giới hạn (Persistent)" tone="info" />
              </div>
            </div>
          </Panel>

          <ApiTokenInventory api={api} />
        </>
      )}

      {isSessions && <SessionInventory api={api} />}
    </div>
  );
}

type SessionState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; sessions: WebSessionSummary[] };

function SessionInventory({
  api,
  compact = false,
}: {
  api: WebApiClient;
  compact?: boolean;
}) {
  const [_revision, setRevision] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [state, setState] = useState<SessionState>({ status: "loading" });

  useEffect(() => {
    const controller = new AbortController();
    setState({ status: "loading" });
    api.auth
      .sessions(controller.signal)
      .then((sessions) => setState({ status: "ready", sessions }))
      .catch((error) => {
        if (!controller.signal.aborted) {
          setState({ status: "error", message: errorMessage(error) });
        }
      });
    return () => controller.abort();
  }, [api]);

  const revoke = async (session: WebSessionSummary) => {
    setBusy(session.id);
    try {
      await api.auth.revokeSession(session.id, new AbortController().signal);
      if (session.current) {
        navigateToLogin();
      } else {
        setRevision((value) => value + 1);
      }
    } finally {
      setBusy(null);
    }
  };

  const revokeAllOthers = async () => {
    if (state.status !== "ready") return;
    const others = state.sessions.filter((s) => !s.current && !s.revokedAt);
    if (others.length === 0) return;
    setBusy("all-others");
    try {
      for (const session of others) {
        await api.auth.revokeSession(session.id, new AbortController().signal);
      }
      setRevision((value) => value + 1);
    } finally {
      setBusy(null);
    }
  };

  if (state.status === "loading") {
    return (
      <Panel
        aria-live="polite"
        title="Phiên đăng nhập và thiết bị kết nối"
        description="Đang kiểm tra danh sách phiên làm việc bảo mật…"
      />
    );
  }

  if (state.status === "error") {
    return (
      <Panel
        actions={
          <Button onClick={() => setRevision((v) => v + 1)}>Thử lại</Button>
        }
        title="Không thể tải danh sách phiên"
        description={state.message}
      />
    );
  }

  const activeSessions = state.sessions.filter((s) => !s.revokedAt);
  const displaySessions = compact ? state.sessions.slice(0, 3) : state.sessions;

  return (
    <Panel
      actions={
        <div style={{ display: "flex", gap: spacing[2], alignItems: "center" }}>
          <Status
            label={`${activeSessions.length} phiên đang hoạt động`}
            tone="info"
          />
          {!compact && activeSessions.filter((s) => !s.current).length > 0 && (
            <Button
              disabled={busy !== null}
              loading={busy === "all-others"}
              onClick={revokeAllOthers}
              variant="danger"
            >
              Đăng xuất các thiết bị khác
            </Button>
          )}
        </div>
      }
      title="Phiên đăng nhập & Thiết bị (Sessions & Devices)"
      description="Quản lý và thu hồi phiên làm việc trên trình duyệt Web, BugLogin Desktop Client hoặc API Tokens."
    >
      <div style={{ display: "grid" }}>
        {displaySessions.map((session) => {
          const isCurrent = Boolean(session.current);
          const isRevoked = Boolean(session.revokedAt);
          return (
            <div
              key={session.id}
              style={{
                alignItems: "center",
                borderBottom: `1px solid ${colors.border.subtle}`,
                display: "grid",
                gap: spacing[3],
                gridTemplateColumns: "minmax(0, 1fr) auto",
                padding: `${spacing[3]}px 0`,
              }}
            >
              <div style={{ minWidth: 0 }}>
                <div
                  style={{
                    alignItems: "center",
                    display: "flex",
                    flexWrap: "wrap",
                    gap: spacing[2],
                  }}
                >
                  <strong style={typography.bodySmall}>
                    {session.deviceId ?? `Thiết bị ${session.kind ?? "Web"}`}
                  </strong>
                  {isCurrent && (
                    <Status label="Phiên hiện tại (Current)" tone="success" />
                  )}
                  {isRevoked && (
                    <Status label="Đã thu hồi (Revoked)" tone="neutral" />
                  )}
                  {!isCurrent && !isRevoked && (
                    <Status label="Đang hoạt động" tone="info" />
                  )}
                </div>
                <div
                  style={{
                    color: colors.text.muted,
                    marginTop: spacing[1],
                    ...typography.caption,
                  }}
                >
                  ID phiên: <code>{session.id.slice(0, 16)}…</code> · Hết hạn:{" "}
                  {formatTime(session.expiresAt)}
                </div>
              </div>
              <Button
                disabled={isRevoked}
                loading={busy === session.id}
                onClick={() => revoke(session)}
                variant={isCurrent ? "danger" : "secondary"}
              >
                {isCurrent ? "Đăng xuất phiên này" : "Thu hồi quyền (Revoke)"}
              </Button>
            </div>
          );
        })}
      </div>

      {compact && state.sessions.length > 3 && (
        <div style={{ marginTop: spacing[3], textAlign: "center" }}>
          <a
            href="/account/sessions"
            style={{
              ...typography.bodySmall,
              color: colors.brand[700],
              fontWeight: 650,
            }}
          >
            Xem toàn bộ {state.sessions.length} phiên thiết bị →
          </a>
        </div>
      )}
    </Panel>
  );
}

function formatTime(epochSeconds: number) {
  if (!epochSeconds) return "Vô thời hạn";
  return new Date(epochSeconds * 1000).toLocaleString();
}

function navigateToLogin() {
  if (typeof window === "undefined") return;
  window.history.replaceState(null, "", "/login");
  window.dispatchEvent(new PopStateEvent("popstate"));
}

function useAccountState(
  load: (signal: AbortSignal) => Promise<AccountSummary>,
  online: boolean,
) {
  const [_revision, setRevision] = useState(0);
  const [state, setState] = useState<AccountLoadState>({ status: "loading" });
  const retry = useCallback(() => setRevision((value) => value + 1), []);

  useEffect(() => {
    const controller = new AbortController();
    setState({ status: "loading" });
    load(controller.signal)
      .then((data) =>
        setState(data ? { status: "ready", data } : { status: "empty" }),
      )
      .catch((error) => {
        if (controller.signal.aborted) return;
        if (!online) setState({ status: "offline" });
        else if (
          error instanceof WebApiError &&
          [401, 403].includes(error.problem.status)
        )
          setState({ status: "forbidden" });
        else
          setState({
            status: "error",
            message: errorMessage(error),
            correlationId:
              error instanceof WebApiError
                ? error.problem.correlationId
                : "unknown",
          });
      });
    return () => controller.abort();
  }, [load, online]);

  return { retry, state };
}

type ApiTokenState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; tokens: ApiTokenSummary[] };

function ApiTokenInventory({ api }: { api: WebApiClient }) {
  const [_revision, setRevision] = useState(0);
  const [state, setState] = useState<ApiTokenState>({ status: "loading" });
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState("profile.read:read,workspace.read:read");
  const [createdToken, setCreatedToken] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    setState({ status: "loading" });
    api.auth
      .apiTokens(controller.signal)
      .then((tokens) => setState({ status: "ready", tokens }))
      .catch((error) => {
        if (!controller.signal.aborted) {
          setState({ status: "error", message: errorMessage(error) });
        }
      });
    return () => controller.abort();
  }, [api]);

  const create = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setBusy("create");
    try {
      const result = await api.auth.createApiToken(
        {
          name: name.trim(),
          scopes: scopes
            .split(",")
            .map((value) => value.trim())
            .filter(Boolean),
          expiresAt: Math.floor(Date.now() / 1000) + 90 * 24 * 60 * 60,
        },
        new AbortController().signal,
      );
      setCreatedToken(result.token);
      setName("");
      setRevision((value) => value + 1);
    } catch (error) {
      setState({ status: "error", message: errorMessage(error) });
    } finally {
      setBusy(null);
    }
  };

  const copyToken = () => {
    if (!createdToken) return;
    navigator.clipboard.writeText(createdToken);
    setCopied(true);
    setTimeout(() => setCopied(false), 2500);
  };

  const revoke = async (token: ApiTokenSummary) => {
    setBusy(token.id);
    try {
      await api.auth.revokeApiToken(token.id, new AbortController().signal);
      setRevision((value) => value + 1);
    } catch (error) {
      setState({ status: "error", message: errorMessage(error) });
    } finally {
      setBusy(null);
    }
  };

  return (
    <Panel
      title="Mã truy cập API (API Tokens)"
      description="Tạo và quản lý các token định danh tự động hóa dành cho CI/CD, bot hoặc tích hợp bên thứ ba."
    >
      <form
        onSubmit={create}
        style={{ display: "grid", gap: spacing[3], marginBottom: spacing[4] }}
      >
        <Field
          label="Tên Token (Token Name)"
          onChange={(event) => setName(event.target.value)}
          placeholder="Ví dụ: CI/CD Pipeline Automation, Profile Sync Bot"
          required
          value={name}
        />
        <Field
          help="Danh sách các quyền cách nhau bởi dấu phẩy. Ví dụ: profile.read:read, profile.write:write, proxy.read:read"
          label="Phạm vi truy cập (Scopes)"
          onChange={(event) => setScopes(event.target.value)}
          required
          value={scopes}
        />
        <Button
          disabled={!name.trim() || !scopes.trim()}
          loading={busy === "create"}
          type="submit"
          variant="primary"
        >
          Tạo API Token mới (Create Token)
        </Button>
      </form>

      {createdToken && (
        <div
          style={{
            background: colors.surface.sunken,
            border: `1px solid ${colors.semantic.warning.border}`,
            borderRadius: 6,
            padding: spacing[3],
            marginBottom: spacing[4],
          }}
        >
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
            }}
          >
            <Status
              label="LƯU Ý: Mã Token chỉ hiển thị MỘT LẦN DUY NHẤT. Hãy sao chép và lưu vào vault bảo mật!"
              tone="warning"
              live
            />
            <Button onClick={copyToken} variant="secondary">
              {copied ? "✓ Đã sao chép" : "Sao chép mã Token"}
            </Button>
          </div>
          <code
            style={{
              background: colors.canvas.default,
              borderRadius: 4,
              display: "block",
              marginTop: spacing[2],
              overflowWrap: "anywhere",
              padding: spacing[2],
              fontFamily: "monospace",
              fontSize: "13px",
              border: `1px solid ${colors.border.subtle}`,
            }}
          >
            {createdToken}
          </code>
        </div>
      )}

      {state.status === "loading" && (
        <p style={{ ...typography.bodySmall, color: colors.text.muted }}>
          Đang tải danh sách API Tokens…
        </p>
      )}

      {state.status === "error" && (
        <Status label={state.message} tone="danger" live />
      )}

      {state.status === "ready" && (
        <div style={{ display: "grid" }}>
          {state.tokens.length === 0 ? (
            <p style={{ ...typography.bodySmall, color: colors.text.muted }}>
              Chưa có API Token nào được khởi tạo.
            </p>
          ) : (
            state.tokens.map((token) => (
              <div
                key={token.id}
                style={{
                  alignItems: "center",
                  borderBottom: `1px solid ${colors.border.subtle}`,
                  display: "grid",
                  gap: spacing[3],
                  gridTemplateColumns: "minmax(0, 1fr) auto",
                  padding: `${spacing[3]}px 0`,
                }}
              >
                <div style={{ minWidth: 0 }}>
                  <div
                    style={{
                      display: "flex",
                      gap: spacing[2],
                      alignItems: "center",
                    }}
                  >
                    <strong style={typography.bodySmall}>{token.name}</strong>
                    {token.revokedAt ? (
                      <Status label="Đã thu hồi" tone="neutral" />
                    ) : (
                      <Status label="Đang hoạt động" tone="success" />
                    )}
                  </div>
                  <div
                    style={{
                      color: colors.text.muted,
                      marginTop: spacing[1],
                      ...typography.caption,
                    }}
                  >
                    Phạm vi: {token.scopes.join(", ")} · Hết hạn:{" "}
                    {formatTime(token.expiresAt)} · ID: {token.id.slice(0, 12)}
                  </div>
                </div>
                <Button
                  disabled={token.revokedAt !== null}
                  loading={busy === token.id}
                  onClick={() => revoke(token)}
                  variant="danger"
                >
                  Thu hồi (Revoke)
                </Button>
              </div>
            ))
          )}
        </div>
      )}
    </Panel>
  );
}
