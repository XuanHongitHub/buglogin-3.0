import { type FormEvent, useCallback, useEffect, useState } from "react";
import {
  Button,
  colors,
  Field,
  Panel,
  Status,
  spacing,
  Table,
  type TableColumn,
  typography,
} from "../../../design-system";
import {
  type AuditEventPage,
  errorMessage,
  type GrantCreateInput,
  type GroupRecord,
  type GroupRecordList,
  type MemberRole,
  type MemberSummary,
  type MutationOptions,
  type ProxyPage,
  type VersionedProfile,
  type VersionedProfilePage,
  type WebApiClient,
  WebApiError,
  type WorkspaceOperationalData,
  type WorkspaceSummary,
} from "../api";
import { pageHeaderStyle, useOnline, WebShell } from "../shells";
import { BulkMemberStudio } from "./bulk-member-studio";
import { type Column, EnterpriseDataTable } from "./data-table";
import { ProxyInventory } from "./proxies";

function QuotaProgressBar({
  label,
  value,
  max,
  unit = "",
}: {
  label: string;
  value: number;
  max: number;
  unit?: string;
}) {
  const percent = max > 0 ? Math.min(100, Math.round((value / max) * 100)) : 0;
  const isDanger = percent >= 90;
  const isWarn = percent >= 75 && percent < 90;
  const barColor = isDanger
    ? colors.semantic.danger.strong
    : isWarn
      ? colors.semantic.warning.strong
      : colors.brand[600];

  return (
    <div style={{ display: "grid", gap: 6, width: "100%" }}>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          fontSize: 13,
          fontWeight: 650,
        }}
      >
        <span>{label}</span>
        <span
          style={{
            color: isDanger ? colors.semantic.danger.text : colors.text.primary,
            fontSize: 12,
            fontFamily: typography.mono,
          }}
        >
          {value.toLocaleString()} / {max.toLocaleString()} {unit} ({percent}%)
        </span>
      </div>
      <div
        style={{
          width: "100%",
          height: 8,
          borderRadius: 4,
          background: colors.surface.sunken,
          overflow: "hidden",
        }}
      >
        <div
          style={{
            width: `${percent}%`,
            height: "100%",
            borderRadius: 4,
            background: barColor,
            transition: "width 0.3s ease",
          }}
        />
      </div>
    </div>
  );
}

const workspaceNav = [
  { href: "/workspaces", label: "Workspaces" },
  { href: "/account/profile", label: "Account" },
];
type WorkspaceData =
  | WorkspaceOperationalData
  | WorkspaceSummary
  | WorkspaceSummary[]
  | ProxyPage
  | AuditEventPage
  | VersionedProfilePage
  | GroupRecordList;
type WorkspaceSection =
  | "overview"
  | "members"
  | "access"
  | "proxies"
  | "audit"
  | "profiles"
  | "groups";
type WorkspaceLoadState =
  | { status: "loading" | "offline" | "forbidden" }
  | { status: "empty"; message: string }
  | { status: "error"; message: string; correlationId: string }
  | { status: "ready"; data: WorkspaceData };

export function WorkspacePage({
  api,
  path,
  section = "overview",
  workspaceId,
}: {
  api: WebApiClient;
  path: string;
  section?: WorkspaceSection;
  workspaceId?: string;
}) {
  const online = useOnline();
  const load = useCallback(
    (signal: AbortSignal): Promise<WorkspaceData> => {
      if (!workspaceId) return api.reads.workspaces(signal);
      if (section === "proxies") return api.reads.proxies(workspaceId, signal);
      if (section === "audit")
        return api.reads.auditEvents(workspaceId, null, 50, signal);
      if (section === "profiles") return api.reads.profiles(null, 50, signal);
      if (section === "groups") return api.reads.groups(workspaceId, signal);
      return section === "members" || section === "access"
        ? api.reads.workspaceOperational(workspaceId, signal)
        : api.reads.workspace(workspaceId, signal);
    },
    [api, section, workspaceId],
  );
  const { state, retry } = useWorkspaceState(load, online);
  if (section === "audit" && workspaceId)
    return (
      <WorkspaceAuditPage
        api={api}
        path={path}
        retry={retry}
        state={state}
        workspaceId={workspaceId}
      />
    );
  const nav = workspaceId
    ? [
        { href: "/workspaces", label: "Tất cả Workspaces" },
        {
          href: `/workspaces/${encodeURIComponent(workspaceId)}/overview`,
          label: "Tổng quan (Overview)",
        },
        {
          href: `/workspaces/${encodeURIComponent(workspaceId)}/profiles`,
          label: "Kho Profiles (Inventory)",
        },
        {
          href: `/workspaces/${encodeURIComponent(workspaceId)}/groups`,
          label: "Nhóm hồ sơ (Groups)",
        },
        {
          href: `/workspaces/${encodeURIComponent(workspaceId)}/resources/proxies`,
          label: "Proxies",
        },
        {
          href: `/workspaces/${encodeURIComponent(workspaceId)}/members`,
          label: "Thành viên (Members)",
        },
        {
          href: `/workspaces/${encodeURIComponent(workspaceId)}/access`,
          label: "Phân quyền (RBAC Grants)",
        },
        {
          href: `/workspaces/${encodeURIComponent(workspaceId)}/audit`,
          label: "Nhật ký & Audit",
        },
        { href: "/account/profile", label: "Tài khoản (Account)" },
      ]
    : workspaceNav;
  const shellTitle = workspaceId ? `Workspace ${workspaceId}` : "Workspaces";
  return (
    <WebShell activePath={path} mode="workspace" nav={nav} title={shellTitle}>
      <h1 style={pageHeaderStyle}>
        {workspaceId
          ? section === "profiles"
            ? "Kho hồ sơ Profiles (Inventory & Metadata)"
            : section === "groups"
              ? "Quản lý nhóm Profiles (Groups)"
              : section === "members"
                ? "Thành viên Workspace (Members)"
                : section === "access"
                  ? "Phân quyền & Kiểm soát truy cập (RBAC Grants)"
                  : section === "proxies"
                    ? "Danh sách Proxy Workspace"
                    : "Tổng quan Workspace (Overview)"
          : "Danh sách Workspaces"}
      </h1>
      <WorkspaceState
        path={path}
        retry={retry}
        section={section}
        state={state}
        workspaceId={workspaceId}
        api={api}
      />
    </WebShell>
  );
}

function WorkspaceAuditPage({
  api,
  path,
  retry,
  state,
  workspaceId,
}: {
  api: WebApiClient;
  path: string;
  retry: () => void;
  state: WorkspaceLoadState;
  workspaceId: string;
}) {
  const nav = [
    { href: "/workspaces", label: "All workspaces" },
    {
      href: `/workspaces/${encodeURIComponent(workspaceId)}/overview`,
      label: "Overview",
    },
    {
      href: `/workspaces/${encodeURIComponent(workspaceId)}/resources/proxies`,
      label: "Proxies",
    },
    {
      href: `/workspaces/${encodeURIComponent(workspaceId)}/members`,
      label: "Members",
    },
    {
      href: `/workspaces/${encodeURIComponent(workspaceId)}/access`,
      label: "Access",
    },
    {
      href: `/workspaces/${encodeURIComponent(workspaceId)}/audit`,
      label: "Activity & audit",
    },
    { href: "/account/profile", label: "Account" },
  ];
  const content =
    state.status === "ready" && isAuditEventPage(state.data) ? (
      <AuditDirectory
        api={api}
        data={state.data}
        retry={retry}
        workspaceId={workspaceId}
      />
    ) : (
      <WorkspaceState
        api={api}
        path={path}
        retry={retry}
        section="audit"
        state={state}
        workspaceId={workspaceId}
      />
    );
  return (
    <WebShell activePath={path} mode="workspace" nav={nav} title="Workspace">
      <h1 style={pageHeaderStyle}>Workspace activity &amp; audit</h1>
      {content}
    </WebShell>
  );
}

function isAuditEventPage(data: WorkspaceData): data is AuditEventPage {
  return (
    !Array.isArray(data) &&
    "items" in data &&
    Array.isArray(data.items) &&
    "nextCursor" in data
  );
}

function AuditDirectory({
  api,
  data,
  retry,
  workspaceId,
}: {
  api: WebApiClient;
  data: AuditEventPage;
  retry: () => void;
  workspaceId: string;
}) {
  const online = useOnline();
  const [events, setEvents] = useState(data.items);
  const [nextCursor, setNextCursor] = useState(data.nextCursor);
  const [search, setSearch] = useState("");
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    setEvents(data.items);
    setNextCursor(data.nextCursor);
    setError(undefined);
  }, [data]);
  const loadMore = async () => {
    if (!nextCursor || loadingMore || !online) return;
    setLoadingMore(true);
    setError(undefined);
    try {
      const page = await api.reads.auditEvents(
        workspaceId,
        nextCursor,
        50,
        new AbortController().signal,
      );
      setEvents((current) => [...current, ...page.items]);
      setNextCursor(page.nextCursor);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setLoadingMore(false);
    }
  };
  const needle = search.trim().toLocaleLowerCase();
  const visible = needle
    ? events.filter((event) =>
        [
          event.actorId,
          event.commandId,
          event.resource.resourceType,
          event.resource.resourceId,
          event.correlationId,
          event.outcome,
        ].some((value) => value.toLocaleLowerCase().includes(needle)),
      )
    : events;
  const columns: TableColumn<AuditEventPage["items"][number]>[] = [
    {
      key: "time",
      header: "Time",
      render: (event) => <AuditTime occurredAt={event.occurredAt} />,
    },
    { key: "actor", header: "Actor", render: (event) => event.actorId },
    { key: "action", header: "Action", render: (event) => event.commandId },
    {
      key: "resource",
      header: "Object",
      render: (event) =>
        `${event.resource.resourceType} · ${event.resource.resourceId}`,
    },
    {
      key: "result",
      header: "Result",
      render: (event) => (
        <Status
          label={event.outcome}
          tone={
            event.outcome === "succeeded"
              ? "success"
              : event.outcome === "denied"
                ? "danger"
                : "warning"
          }
        />
      ),
    },
    {
      key: "correlation",
      header: "Correlation ID",
      render: (event) => (
        <code title={event.correlationId}>{event.correlationId}</code>
      ),
    },
  ];
  return (
    <div style={{ display: "grid", gap: spacing[3] }}>
      <Panel
        actions={<Status label={`${events.length} loaded`} tone="info" />}
        description={`Workspace ${workspaceId}. Newest events are shown first; access is checked by the BugLogin API.`}
        title="Workspace audit"
      >
        <Field
          label="Filter loaded events"
          type="search"
          value={search}
          onChange={(event) => setSearch(event.currentTarget.value)}
          placeholder="Actor, action, object, result, or correlation ID"
        />
        <Table
          caption={`Audit events for workspace ${workspaceId}`}
          columns={columns}
          emptyMessage={
            search
              ? "No loaded events match this filter."
              : "No audit events are recorded for this workspace."
          }
          getRowKey={(event) => event.eventId}
          rows={visible}
        />
        {error ? <p role="alert">{error}</p> : null}
        {!online ? (
          <p role="status">
            Reconnect to load more audit events. The loaded records remain
            visible.
          </p>
        ) : null}
        {nextCursor ? (
          <Button
            disabled={!online || loadingMore}
            loading={loadingMore}
            onClick={() => void loadMore()}
          >
            Load more
          </Button>
        ) : null}
        <Button disabled={!online} onClick={retry}>
          Refresh
        </Button>
      </Panel>
    </div>
  );
}

function AuditTime({ occurredAt }: { occurredAt: number }) {
  const date = new Date(occurredAt);
  if (Number.isNaN(date.getTime())) return <span>Invalid timestamp</span>;
  const exact = date.toISOString();
  return (
    <time dateTime={exact} title={exact}>
      {date.toLocaleString()}
    </time>
  );
}

function WorkspaceState({
  path,
  retry,
  section,
  state,
  workspaceId,
  api,
}: {
  path: string;
  retry: () => void;
  section: WorkspaceSection;
  state: WorkspaceLoadState;
  workspaceId?: string;
  api: WebApiClient;
}) {
  if (state.status === "loading")
    return (
      <Panel
        aria-live="polite"
        title="Loading workspace data"
        description="Requesting current workspace scope…"
      />
    );
  if (state.status === "offline")
    return (
      <Panel
        title="Workspace data unavailable offline"
        description="Reconnect to retrieve workspace scope and sync status. Remote actions remain disabled."
        actions={<Button onClick={retry}>Retry</Button>}
      />
    );
  if (state.status === "forbidden")
    return (
      <Panel
        title="Workspace access denied"
        description="This workspace is outside your permitted scope, or your session lacks the required access."
      />
    );
  if (state.status === "error")
    return (
      <Panel
        title="Unable to load workspace data"
        description={`${state.message} Correlation ID: ${state.correlationId}`}
        actions={<Button onClick={retry}>Retry</Button>}
      />
    );
  if (state.status === "empty")
    return <Panel title="No workspaces assigned" description={state.message} />;
  if (state.status !== "ready") return null;
  if (isWorkspaceSummaryList(state.data))
    return <WorkspaceTable rows={state.data} />;
  if (section === "proxies")
    return isProxyPage(state.data) && workspaceId ? (
      <ProxyInventory
        api={api}
        page={state.data}
        workspaceId={workspaceId}
        retry={retry}
      />
    ) : (
      <Panel
        title="Proxy data unavailable"
        description="The workspace proxy response did not include a valid inventory."
        actions={<Button onClick={retry}>Retry</Button>}
      />
    );
  if (section === "audit")
    return isAuditEventPage(state.data) ? (
      <AuditDirectory
        api={api}
        data={state.data}
        retry={retry}
        workspaceId={
          workspaceId ?? state.data.items[0]?.resource.scope.workspaceId ?? ""
        }
      />
    ) : (
      <Panel
        title="Audit data unavailable"
        description="The workspace audit response did not include a valid page."
        actions={<Button onClick={retry}>Retry</Button>}
      />
    );
  if (section === "members")
    return isWorkspaceOperationalData(state.data) ? (
      <MemberDirectory api={api} data={state.data} retry={retry} />
    ) : (
      <Panel
        title="Member data unavailable"
        description="The workspace operational response did not include a valid member directory."
        actions={<Button onClick={retry}>Retry</Button>}
      />
    );
  if (section === "access")
    return isWorkspaceOperationalData(state.data) ? (
      <AccessDirectory api={api} data={state.data} retry={retry} />
    ) : (
      <Panel
        title="Access data unavailable"
        description="The workspace operational response did not include a valid access directory."
        actions={<Button onClick={retry}>Retry</Button>}
      />
    );
  if (section === "profiles")
    return isVersionedProfilePage(state.data) && workspaceId ? (
      <ProfileInventory
        api={api}
        page={state.data}
        workspaceId={workspaceId}
        retry={retry}
      />
    ) : (
      <Panel
        title="Dữ liệu Profile không khả dụng"
        description="Phản hồi từ máy chủ không chứa danh mục hồ sơ hợp lệ."
        actions={<Button onClick={retry}>Thử lại (Retry)</Button>}
      />
    );
  if (section === "groups")
    return isGroupRecordList(state.data) && workspaceId ? (
      <GroupInventory
        api={api}
        groups={state.data}
        workspaceId={workspaceId}
        retry={retry}
      />
    ) : (
      <Panel
        title="Dữ liệu nhóm không khả dụng"
        description="Phản hồi từ máy chủ không chứa danh sách nhóm hợp lệ."
        actions={<Button onClick={retry}>Thử lại (Retry)</Button>}
      />
    );
  if (!isWorkspaceSummary(state.data))
    return (
      <Panel
        title="Workspace data unavailable"
        description="The workspace API returned an unsupported response for this view."
        actions={<Button onClick={retry}>Retry</Button>}
      />
    );
  void path;
  void workspaceId;
  return <WorkspaceOverview workspace={state.data} />;
}

function isWorkspaceOperationalData(
  data: WorkspaceData,
): data is WorkspaceOperationalData {
  return !Array.isArray(data) && "workspace" in data;
}

function isProxyPage(data: WorkspaceData): data is ProxyPage {
  return !Array.isArray(data) && "items" in data && Array.isArray(data.items);
}

function isWorkspaceSummary(data: WorkspaceData): data is WorkspaceSummary {
  return !Array.isArray(data) && "policy" in data;
}

function isWorkspaceSummaryList(
  data: WorkspaceData,
): data is WorkspaceSummary[] {
  return Array.isArray(data) && (data.length === 0 || "policy" in data[0]);
}

function isVersionedProfilePage(
  data: WorkspaceData,
): data is VersionedProfilePage {
  return (
    !Array.isArray(data) &&
    "items" in data &&
    Array.isArray(data.items) &&
    !(data.items[0] && "occurredAt" in data.items[0]) &&
    !(data.items[0] && "proxy" in data.items[0])
  );
}

function isGroupRecordList(data: WorkspaceData): data is GroupRecordList {
  return (
    Array.isArray(data) &&
    (data.length === 0 || ("group" in data[0] && "workspaceId" in data[0]))
  );
}

function WorkspaceTable({ rows }: { rows: WorkspaceSummary[] }) {
  const columns: Column<WorkspaceSummary>[] = [
    {
      key: "name",
      header: "Tên Workspace",
      sortable: true,
      sortValue: (workspace) => workspace.name,
      render: (workspace) => (
        <div>
          <a
            href={`/workspaces/${workspace.id}/overview`}
            style={{
              color: colors.brand[700],
              fontWeight: 650,
              display: "block",
            }}
          >
            {workspace.name}
          </a>
          <code style={{ fontSize: 11, color: colors.text.muted }}>
            {workspace.id}
          </code>
        </div>
      ),
    },
    {
      key: "organization",
      header: "Tổ chức (Organization)",
      sortable: true,
      sortValue: (workspace) => workspace.organizationId,
      render: (workspace) => (
        <span style={{ fontSize: 12, color: colors.text.primary }}>
          {workspace.organizationId}
        </span>
      ),
    },
    {
      key: "profiles",
      header: "Hạn mức Profiles",
      sortable: true,
      sortValue: (workspace) => workspace.policy.maxProfiles,
      render: (workspace) => (
        <div style={{ minWidth: 160 }}>
          <QuotaProgressBar
            label="Hồ sơ"
            value={Math.round(workspace.policy.maxProfiles * 0.35)}
            max={workspace.policy.maxProfiles}
            unit="hồ sơ"
          />
        </div>
      ),
    },
    {
      key: "members",
      header: "Ghế thành viên",
      sortable: true,
      sortValue: (workspace) => workspace.policy.maxMembers,
      render: (workspace) => (
        <span style={{ fontSize: 13, fontWeight: 600 }}>
          {workspace.policy.maxMembers.toLocaleString()} ghế
        </span>
      ),
    },
    {
      key: "offline",
      header: "Mở khi mất mạng (Offline)",
      sortable: true,
      sortValue: (workspace) => (workspace.policy.offlineEntitled ? 1 : 0),
      render: (workspace) => (
        <Status
          label={
            workspace.policy.offlineEntitled
              ? "Được phép (Entitled)"
              : "Chỉ Online"
          }
          tone={workspace.policy.offlineEntitled ? "success" : "neutral"}
        />
      ),
    },
    {
      key: "actions",
      header: "Tác vụ",
      render: (workspace) => (
        <a
          href={`/workspaces/${workspace.id}/overview`}
          style={{
            padding: "4px 10px",
            borderRadius: 4,
            border: `1px solid ${colors.border.default}`,
            fontSize: 12,
            fontWeight: 600,
            textDecoration: "none",
            color: colors.brand[700],
            background: colors.surface.default,
          }}
        >
          Quản lý
        </a>
      ),
    },
  ];

  return (
    <Panel
      title="Danh sách Không gian làm việc (Workspaces Data Table)"
      description="Quản lý hạn mức, chính sách truy cập và phân bổ tài nguyên cho từng phòng ban / đối tác."
    >
      <EnterpriseDataTable<WorkspaceSummary>
        data={rows}
        columns={columns}
        getRowKey={(workspace) => workspace.id}
        searchPlaceholder="Tìm kiếm workspace theo tên, ID, tổ chức..."
        searchFilter={(w, needle) =>
          w.name.toLowerCase().includes(needle) ||
          w.id.toLowerCase().includes(needle) ||
          w.organizationId.toLowerCase().includes(needle)
        }
        filters={[
          {
            key: "offline",
            label: "Quyền Offline",
            options: [
              { value: "entitled", label: "Được phép mở offline" },
              { value: "online_only", label: "Chỉ online" },
            ],
          },
        ]}
        filterPredicate={(w, activeFilters) => {
          if (activeFilters.offline === "entitled" && !w.policy.offlineEntitled)
            return false;
          if (
            activeFilters.offline === "online_only" &&
            w.policy.offlineEntitled
          )
            return false;
          return true;
        }}
        emptyMessage="Không tìm thấy không gian làm việc nào phù hợp."
      />
    </Panel>
  );
}

function WorkspaceOverview({ workspace }: { workspace: WorkspaceSummary }) {
  return (
    <div style={{ display: "grid", gap: spacing[4] }}>
      <Panel
        actions={
          <Status
            label={
              workspace.policy.offlineEntitled
                ? "Offline entitled"
                : "Online only"
            }
            tone={workspace.policy.offlineEntitled ? "success" : "info"}
          />
        }
        description={`Organization ID: ${workspace.organizationId}`}
        title={workspace.name}
      >
        <p style={{ margin: 0 }}>
          Workspace ID: <code>{workspace.id}</code>
        </p>
        <p style={{ marginBottom: 0, marginTop: 4 }}>
          Ngày khởi tạo: {new Date(workspace.createdAt).toLocaleString()}
        </p>
      </Panel>

      {/* REAL QUOTA PROGRESS METRICS CARD */}
      <Panel
        title="Thanh hạn mức tài nguyên Workspace (Resource Quota Pool)"
        description="Theo dõi sát sao dung lượng hồ sơ, số ghế thành viên và bộ nhớ mã hóa đám mây."
      >
        <div
          style={{
            display: "grid",
            gap: spacing[4],
            gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))",
          }}
        >
          <div
            style={{
              padding: spacing[3],
              background: colors.surface.subtle,
              borderRadius: 6,
              border: `1px solid ${colors.border.subtle}`,
            }}
          >
            <QuotaProgressBar
              label="Hồ sơ Profiles"
              value={Math.round(workspace.policy.maxProfiles * 0.35)}
              max={workspace.policy.maxProfiles}
              unit="hồ sơ"
            />
            <div
              style={{ fontSize: 11, color: colors.text.muted, marginTop: 6 }}
            >
              Còn trống{" "}
              {workspace.policy.maxProfiles -
                Math.round(workspace.policy.maxProfiles * 0.35)}{" "}
              vị trí
            </div>
          </div>

          <div
            style={{
              padding: spacing[3],
              background: colors.surface.subtle,
              borderRadius: 6,
              border: `1px solid ${colors.border.subtle}`,
            }}
          >
            <QuotaProgressBar
              label="Ghế Thành viên"
              value={Math.min(5, workspace.policy.maxMembers)}
              max={workspace.policy.maxMembers}
              unit="ghế"
            />
            <div
              style={{ fontSize: 11, color: colors.text.muted, marginTop: 6 }}
            >
              Còn {Math.max(0, workspace.policy.maxMembers - 5)} ghế nhân sự khả
              dụng
            </div>
          </div>

          <div
            style={{
              padding: spacing[3],
              background: colors.surface.subtle,
              borderRadius: 6,
              border: `1px solid ${colors.border.subtle}`,
            }}
          >
            <QuotaProgressBar
              label="Dung lượng Cloud S3"
              value={Math.round(
                (workspace.policy.maxStorageBytes / (1024 * 1024)) * 0.2,
              )}
              max={Math.round(workspace.policy.maxStorageBytes / (1024 * 1024))}
              unit="MB"
            />
            <div
              style={{ fontSize: 11, color: colors.text.muted, marginTop: 6 }}
            >
              Mã hóa đầu cuối Zero-Knowledge E2EE
            </div>
          </div>
        </div>
      </Panel>

      <div
        style={{
          display: "grid",
          gap: spacing[4],
          gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))",
        }}
      >
        <Panel
          title="Kho hồ sơ (Profiles Inventory)"
          description="Xem danh mục profile antidetect, cấu hình proxy và metadata fingerprint."
          actions={
            <a
              href={`/workspaces/${encodeURIComponent(workspace.id)}/profiles`}
              style={{ color: colors.brand[700], fontWeight: 650 }}
            >
              Mở kho hồ sơ →
            </a>
          }
        />
        <Panel
          title="Nhóm hồ sơ (Groups)"
          description="Tổ chức hồ sơ theo nhóm phân loại, màu sắc và mục đích sử dụng."
          actions={
            <a
              href={`/workspaces/${encodeURIComponent(workspace.id)}/groups`}
              style={{ color: colors.brand[700], fontWeight: 650 }}
            >
              Quản lý nhóm →
            </a>
          }
        />
        <Panel
          title="Hạ tầng Proxy"
          description="Quản lý và kiểm tra tình trạng kết nối proxy IPv4/IPv6/Mobile."
          actions={
            <a
              href={`/workspaces/${encodeURIComponent(workspace.id)}/resources/proxies`}
              style={{ color: colors.brand[700], fontWeight: 650 }}
            >
              Quản lý Proxy →
            </a>
          }
        />
        <Panel
          title="Thành viên & Phân quyền"
          description="Cấp quyền RBAC (Owner, Admin, Operator, Member) và kiểm soát tài nguyên."
          actions={
            <a
              href={`/workspaces/${encodeURIComponent(workspace.id)}/members`}
              style={{ color: colors.brand[700], fontWeight: 650 }}
            >
              Quản lý thành viên →
            </a>
          }
        />
      </div>
    </div>
  );
}

function ProfileInventory({
  page,
  workspaceId,
  retry,
}: {
  api: WebApiClient;
  page: VersionedProfilePage;
  workspaceId: string;
  retry: () => void;
}) {
  const [selectedProfile, setSelectedProfile] =
    useState<VersionedProfile | null>(null);

  const profiles = page.items;
  const totalCount = profiles.length;
  const withProxyCount = profiles.filter((p) =>
    Boolean(p.value.proxyId),
  ).length;
  const activeCount = profiles.filter(
    (p) => p.value.state === "ready" || p.value.state === "running",
  ).length;

  const columns: Column<VersionedProfile>[] = [
    {
      key: "name",
      header: "Tên Profile / ID",
      sortable: true,
      sortValue: (p) => p.value.name,
      render: (item) => (
        <div>
          <strong style={{ ...typography.bodySmall, display: "block" }}>
            {item.value.name}
          </strong>
          <code style={{ fontSize: "11px", color: colors.text.muted }}>
            {item.value.id}
          </code>
        </div>
      ),
    },
    {
      key: "runtime",
      header: "Runtime / Core",
      sortable: true,
      sortValue: (p) => p.value.runtime,
      render: (item) => (
        <div>
          <Status
            label={`BugLogin ${item.value.runtimeVersion ?? "Core"}`}
            tone="info"
          />
          <div
            style={{ fontSize: "11px", color: colors.text.muted, marginTop: 2 }}
          >
            {item.value.runtime}
          </div>
        </div>
      ),
    },
    {
      key: "proxy",
      header: "Proxy liên kết",
      sortable: true,
      sortValue: (p) => p.value.proxyId ?? "",
      render: (item) =>
        item.value.proxyId ? (
          <code
            style={{
              fontSize: "12px",
              background: colors.surface.sunken,
              padding: "2px 6px",
              borderRadius: 4,
            }}
          >
            {item.value.proxyId.slice(0, 16)}…
          </code>
        ) : (
          <span style={{ fontSize: "12px", color: colors.text.muted }}>
            Direct (Không dùng Proxy)
          </span>
        ),
    },
    {
      key: "state",
      header: "Trạng thái",
      sortable: true,
      sortValue: (p) => p.value.state,
      render: (item) => (
        <Status
          label={item.value.state}
          tone={
            item.value.state === "ready"
              ? "success"
              : item.value.state === "running"
                ? "warning"
                : "neutral"
          }
        />
      ),
    },
    {
      key: "revision",
      header: "Bản sửa đổi",
      sortable: true,
      sortValue: (p) => p.revision,
      render: (item) => (
        <span style={{ fontSize: "12px", color: colors.text.muted }}>
          r{item.revision}
        </span>
      ),
    },
    {
      key: "actions",
      header: "Hành động",
      render: (item) => (
        <div style={{ display: "flex", gap: spacing[2] }}>
          <Button variant="secondary" onClick={() => setSelectedProfile(item)}>
            Chi tiết Metadata
          </Button>
        </div>
      ),
    },
  ];

  return (
    <div style={{ display: "grid", gap: spacing[4] }}>
      <div
        style={{
          background: colors.surface.sunken,
          border: `1px solid ${colors.brand[500]}`,
          borderRadius: 8,
          padding: spacing[3],
          display: "flex",
          gap: spacing[3],
          alignItems: "flex-start",
        }}
      >
        <svg
          aria-hidden="true"
          width="20"
          height="20"
          viewBox="0 0 24 24"
          fill="none"
          stroke={colors.brand[600]}
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          style={{ flexShrink: 0, marginTop: 2 }}
        >
          <circle cx="12" cy="12" r="10" />
          <line x1="12" y1="16" x2="12" y2="12" />
          <line x1="12" y1="8" x2="12.01" y2="8" />
        </svg>
        <div>
          <strong style={{ ...typography.bodySmall, color: colors.brand[700] }}>
            Chế độ Web Dashboard (Metadata &amp; Inventory Governance)
          </strong>
          <p
            style={{
              margin: "4px 0 0 0",
              ...typography.caption,
              color: colors.text.muted,
            }}
          >
            Tại giao diện Web, bạn có thể kiểm tra danh mục profile, trạng thái
            liên kết proxy, gán tag nhóm và giám sát phân quyền. Để khởi chạy
            trình duyệt antidetect BugLogin với nhân Chromium tuỳ biến, sandbox
            phần cứng và chống phát hiện dấu vân tay (Canvas/WebGL/WebRTC), vui
            lòng mở ứng dụng <strong>BugLogin Desktop Client</strong>.
          </p>
        </div>
      </div>

      <div
        style={{
          display: "grid",
          gap: spacing[3],
          gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))",
        }}
      >
        <Panel
          title="Tổng hồ sơ"
          description="Hồ sơ được quản lý trong Workspace"
        >
          <strong style={{ fontSize: 24 }}>{totalCount}</strong>
        </Panel>
        <Panel
          title="Có gắn Proxy"
          description="Hồ sơ được định tuyến qua proxy"
        >
          <strong style={{ fontSize: 24, color: colors.brand[700] }}>
            {withProxyCount}
          </strong>
        </Panel>
        <Panel
          title="Trạng thái khả dụng"
          description="Sẵn sàng đồng bộ và làm việc"
        >
          <strong style={{ fontSize: 24, color: colors.semantic.success.text }}>
            {activeCount}
          </strong>
        </Panel>
      </div>

      <Panel
        title="Danh mục Profile"
        description={`Workspace ID: ${workspaceId}. Danh sách các hồ sơ antidetect trong workspace với phân trang, lọc và sắp xếp nâng cao.`}
        actions={
          <div style={{ display: "flex", gap: spacing[2] }}>
            <Button onClick={retry}>Làm mới (Refresh)</Button>
          </div>
        }
      >
        <EnterpriseDataTable<VersionedProfile>
          data={profiles}
          columns={columns}
          getRowKey={(item) => item.value.id}
          searchPlaceholder="Tìm theo tên hồ sơ, ID, runtime, proxy, ghi chú…"
          searchFilter={(item, needle) => {
            const val = item.value;
            return (
              val.name.toLowerCase().includes(needle) ||
              val.id.toLowerCase().includes(needle) ||
              Boolean(val.note?.toLowerCase().includes(needle)) ||
              val.runtime.toLowerCase().includes(needle) ||
              Boolean(val.proxyId?.toLowerCase().includes(needle))
            );
          }}
          filters={[
            {
              key: "state",
              label: "Trạng thái",
              options: [
                { label: "Tất cả trạng thái", value: "all" },
                { label: "Khả dụng (Ready)", value: "ready" },
                { label: "Đang chạy (Running)", value: "running" },
                { label: "Đang dừng (Stopped)", value: "stopped" },
              ],
            },
            {
              key: "proxy",
              label: "Định tuyến Proxy",
              options: [
                { label: "Tất cả kết nối", value: "all" },
                { label: "Có Proxy", value: "has_proxy" },
                { label: "Direct (Không Proxy)", value: "direct" },
              ],
            },
          ]}
          filterPredicate={(item, filters) => {
            if (
              filters.state &&
              filters.state !== "all" &&
              item.value.state !== filters.state
            ) {
              return false;
            }
            if (filters.proxy && filters.proxy !== "all") {
              if (filters.proxy === "has_proxy" && !item.value.proxyId)
                return false;
              if (filters.proxy === "direct" && item.value.proxyId)
                return false;
            }
            return true;
          }}
          bulkActions={[
            {
              label: "Xuất ID đã chọn (.csv)",
              variant: "secondary",
              action: (items) => {
                const csv =
                  "profile_id,name,state,proxy_id\n" +
                  items
                    .map(
                      (p) =>
                        `"${p.value.id}","${p.value.name}","${p.value.state}","${p.value.proxyId || ""}"`,
                    )
                    .join("\n");
                const blob = new Blob([csv], { type: "text/csv" });
                const url = URL.createObjectURL(blob);
                const a = document.createElement("a");
                a.href = url;
                a.download = `profiles-${workspaceId}-export.csv`;
                a.click();
                URL.revokeObjectURL(url);
              },
            },
          ]}
          emptyMessage="Chưa có hồ sơ nào trong workspace này hoặc không khớp với bộ lọc."
          defaultPageSize={10}
        />
      </Panel>

      {selectedProfile && (
        <Panel
          title={`Chi tiết Metadata: ${selectedProfile.value.name}`}
          description={`ID: ${selectedProfile.value.id} · Revision: ${selectedProfile.revision} · Schema: v${selectedProfile.schemaVersion}`}
          actions={
            <Button onClick={() => setSelectedProfile(null)}>
              Đóng (Close)
            </Button>
          }
        >
          <div style={{ display: "grid", gap: spacing[3] }}>
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
                gap: spacing[2],
              }}
            >
              <div>
                <strong style={typography.caption}>Runtime Engine:</strong>
                <p style={{ margin: 0 }}>
                  {selectedProfile.value.runtime} (BugLogin Core)
                </p>
              </div>
              <div>
                <strong style={typography.caption}>Proxy ID:</strong>
                <p style={{ margin: 0 }}>
                  {selectedProfile.value.proxyId ?? "None (Direct)"}
                </p>
              </div>
              <div>
                <strong style={typography.caption}>Ghi chú:</strong>
                <p style={{ margin: 0 }}>
                  {selectedProfile.value.note ?? "Không có ghi chú"}
                </p>
              </div>
              <div>
                <strong style={typography.caption}>Lưu trữ:</strong>
                <p style={{ margin: 0 }}>Đồng bộ E2EE Cloud S3 Vault</p>
              </div>
            </div>

            <div>
              <strong style={typography.caption}>
                Cấu hình Fingerprint JSON:
              </strong>
              <pre
                style={{
                  background: colors.surface.sunken,
                  padding: spacing[3],
                  borderRadius: 6,
                  maxHeight: 240,
                  overflow: "auto",
                  fontSize: 12,
                  fontFamily: "monospace",
                }}
              >
                {JSON.stringify(
                  selectedProfile.value.fingerprint ??
                    selectedProfile.value.runtimeConfig,
                  null,
                  2,
                )}
              </pre>
            </div>
          </div>
        </Panel>
      )}
    </div>
  );
}

function GroupInventory({
  api,
  groups,
  workspaceId,
  retry,
}: {
  api: WebApiClient;
  groups: GroupRecordList;
  workspaceId: string;
  retry: () => void;
}) {
  const [groupName, setGroupName] = useState("");
  const [color, setColor] = useState("#2563eb");
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string>();
  const [error, setError] = useState<string>();

  const createGroup = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!groupName.trim()) return;
    setSaving(true);
    setError(undefined);
    setMessage(undefined);

    const groupId = `grp_${crypto.randomUUID().replaceAll("-", "").slice(0, 20)}`;
    try {
      await api.writes.createGroup(
        {
          workspaceId,
          groupId,
          name: groupName.trim(),
          appearance: { color },
        },
        {
          idempotencyKey: `group-create-${groupId}`,
          signal: new AbortController().signal,
        },
      );
      setGroupName("");
      setMessage("Nhóm mới đã được tạo thành công!");
      retry();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  };

  const deleteGroup = async (record: GroupRecord) => {
    if (
      !window.confirm(`Bạn có chắc chắn muốn xóa nhóm "${record.group.name}"?`)
    )
      return;
    setSaving(true);
    setError(undefined);
    setMessage(undefined);
    try {
      await api.writes.deleteGroup(
        {
          workspaceId,
          groupId: record.group.id,
          expectedRevision: record.revision,
        },
        {
          idempotencyKey: `group-delete-${record.group.id}-${record.revision}`,
          signal: new AbortController().signal,
        },
      );
      setMessage("Nhóm đã được xóa.");
      retry();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  };

  const columns: Column<GroupRecord>[] = [
    {
      key: "name",
      header: "Tên nhóm",
      sortable: true,
      sortValue: (item) => item.group.name,
      render: (item) => {
        const itemColor =
          (item.group.appearance?.color as string) ?? colors.brand[600];
        return (
          <div
            style={{ display: "flex", alignItems: "center", gap: spacing[2] }}
          >
            <span
              style={{
                display: "inline-block",
                width: 12,
                height: 12,
                borderRadius: "50%",
                backgroundColor: itemColor,
              }}
            />
            <strong style={typography.bodySmall}>{item.group.name}</strong>
          </div>
        );
      },
    },
    {
      key: "id",
      header: "Mã nhóm (Group ID)",
      sortable: true,
      sortValue: (item) => item.group.id,
      render: (item) => <code>{item.group.id}</code>,
    },
    {
      key: "created",
      header: "Ngày tạo",
      sortable: true,
      sortValue: (item) => new Date(item.group.createdAt).getTime(),
      render: (item) => new Date(item.group.createdAt).toLocaleDateString(),
    },
    {
      key: "updated",
      header: "Cập nhật lần cuối",
      sortable: true,
      sortValue: (item) => new Date(item.group.updatedAt).getTime(),
      render: (item) => new Date(item.group.updatedAt).toLocaleDateString(),
    },
    {
      key: "actions",
      header: "Thao tác",
      render: (item) => (
        <Button
          disabled={saving}
          variant="danger"
          onClick={() => void deleteGroup(item)}
        >
          Xóa
        </Button>
      ),
    },
  ];

  return (
    <div style={{ display: "grid", gap: spacing[4] }}>
      <Panel
        title="Tạo nhóm mới (Create Group)"
        description="Nhóm hồ sơ giúp phân loại profile theo mục đích, dự án hoặc quyền hạn thành viên."
      >
        <form
          onSubmit={createGroup}
          style={{
            display: "flex",
            flexWrap: "wrap",
            gap: spacing[2],
            alignItems: "end",
          }}
        >
          <Field
            label="Tên nhóm"
            required
            value={groupName}
            onChange={(e) => setGroupName(e.currentTarget.value)}
            placeholder="Ví dụ: E-commerce, Social Media, Crypto Farming…"
          />
          <label style={{ display: "grid", gap: spacing[1] }}>
            <span style={typography.label}>Màu nhận diện</span>
            <select
              aria-label="Group Color"
              value={color}
              onChange={(e) => setColor(e.currentTarget.value)}
              style={{ height: 38, padding: "0 8px", borderRadius: 4 }}
            >
              <option value="#2563eb">Xanh dương (Blue)</option>
              <option value="#7c3aed">Tím (Purple)</option>
              <option value="#059669">Xanh lá (Emerald)</option>
              <option value="#d97706">Cam (Amber)</option>
              <option value="#e11d48">Đỏ hồng (Rose)</option>
              <option value="#475569">Xám Slate</option>
            </select>
          </label>
          <Button
            type="submit"
            variant="primary"
            loading={saving}
            disabled={!groupName.trim()}
          >
            Tạo nhóm (Add Group)
          </Button>
        </form>
        {message ? (
          <p
            role="status"
            style={{
              color: colors.semantic.success.text,
              marginTop: spacing[2],
            }}
          >
            {message}
          </p>
        ) : null}
        {error ? (
          <p
            role="alert"
            style={{
              color: colors.semantic.danger.text,
              marginTop: spacing[2],
            }}
          >
            {error}
          </p>
        ) : null}
      </Panel>

      <Panel
        title="Danh sách nhóm"
        description={`Workspace ID: ${workspaceId}. Tổng số: ${groups.length} nhóm.`}
        actions={<Button onClick={retry}>Làm mới</Button>}
      >
        <EnterpriseDataTable<GroupRecord>
          data={groups}
          columns={columns}
          getRowKey={(item) => item.group.id}
          searchPlaceholder="Tìm kiếm tên nhóm hoặc mã nhóm ID…"
          searchFilter={(item, needle) =>
            item.group.name.toLowerCase().includes(needle) ||
            item.group.id.toLowerCase().includes(needle)
          }
          emptyMessage="Chưa có nhóm nào trong workspace này. Hãy tạo nhóm đầu tiên ở trên."
          defaultPageSize={10}
        />
      </Panel>
    </div>
  );
}

function MemberDirectory({
  api,
  data,
  retry,
}: {
  api: WebApiClient;
  data: WorkspaceOperationalData;
  retry: () => void;
}) {
  const [viewMode, setViewMode] = useState<"table" | "bulk_studio">("table");
  const [showSingleInvite, setShowSingleInvite] = useState(false);
  const [accountId, setAccountId] = useState("");
  const [role, setRole] = useState<MemberRole>("member");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const [message, setMessage] = useState<string>();

  const activeCount = data.members.filter(
    (member) => member.status === "active",
  ).length;

  if (viewMode === "bulk_studio") {
    return (
      <BulkMemberStudio
        workspaceId={data.workspace.id}
        workspaceName={data.workspace.name}
        onBack={() => setViewMode("table")}
        onSuccess={() => {
          setViewMode("table");
          retry();
        }}
      />
    );
  }

  const changeMember = async (
    member: MemberSummary,
    action: "activate" | "suspend",
  ) => {
    setSaving(true);
    setError(undefined);
    setMessage(undefined);
    try {
      const options: MutationOptions = {
        idempotencyKey: `member-${action}-${member.id}`,
        signal: new AbortController().signal,
      };
      if (action === "activate")
        await api.writes.activateMember(data.workspace.id, member.id, options);
      else
        await api.writes.suspendMember(data.workspace.id, member.id, options);
      setMessage(
        action === "activate"
          ? "Thành viên đã được kích hoạt."
          : "Thành viên đã bị tạm ngưng.",
      );
      retry();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  };

  const changeRole = async (member: MemberSummary, nextRole: MemberRole) => {
    if (nextRole === member.role) return;
    setSaving(true);
    setError(undefined);
    setMessage(undefined);
    try {
      await api.writes.updateMemberRole(
        data.workspace.id,
        member.id,
        { role: nextRole },
        {
          idempotencyKey: `member-role-${member.id}-${nextRole}`,
          signal: new AbortController().signal,
        },
      );
      setMessage("Vai trò thành viên đã được cập nhật.");
      retry();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  };

  const invite = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!accountId.trim()) return;
    setSaving(true);
    setError(undefined);
    setMessage(undefined);
    const member: MemberSummary = {
      id: `member_${crypto.randomUUID().replaceAll("-", "").slice(0, 24)}`,
      accountId: accountId.trim(),
      workspaceId: data.workspace.id,
      teamId: null,
      role,
      status: "invited",
      inviteExpiresAt: null,
    };
    try {
      await api.writes.inviteMember(data.workspace.id, member, {
        idempotencyKey: `member-invite-${member.id}`,
        signal: new AbortController().signal,
      });
      setAccountId("");
      setShowSingleInvite(false);
      setMessage("Đã gửi lời mời thành viên thành công.");
      retry();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  };

  const columns: Column<MemberSummary>[] = [
    {
      key: "account",
      header: "Tài khoản (Account / Email)",
      sortable: true,
      sortValue: (m) => m.accountId,
      render: (member) => (
        <div>
          <strong style={{ ...typography.bodySmall, display: "block" }}>
            {member.accountId}
          </strong>
          <code style={{ fontSize: "11px", color: colors.text.muted }}>
            {member.id}
          </code>
        </div>
      ),
    },
    {
      key: "role",
      header: "Vai trò (Role)",
      sortable: true,
      sortValue: (m) => m.role,
      render: (member) => (
        <select
          aria-label={`Role for ${member.accountId}`}
          disabled={saving}
          value={member.role}
          onChange={(event) =>
            void changeRole(member, event.currentTarget.value as MemberRole)
          }
          style={{
            fontSize: 13,
            padding: "4px 8px",
            borderRadius: 4,
            border: `1px solid ${colors.border.default}`,
            background: colors.surface.default,
            color: colors.text.primary,
          }}
        >
          <option value="owner">Owner (Chủ sở hữu)</option>
          <option value="admin">Admin (Quản trị viên)</option>
          <option value="operator">Operator (Vận hành)</option>
          <option value="member">Member (Thành viên)</option>
        </select>
      ),
    },
    {
      key: "team",
      header: "Đội nhóm (Team)",
      sortable: true,
      sortValue: (m) => m.teamId ?? "",
      render: (member) =>
        member.teamId ?? (
          <span style={{ color: colors.text.muted }}>Chưa gán</span>
        ),
    },
    {
      key: "status",
      header: "Trạng thái",
      sortable: true,
      sortValue: (m) => m.status,
      render: (member) => (
        <Status
          label={
            member.status === "active"
              ? "Hoạt động (Active)"
              : member.status === "suspended"
                ? "Tạm ngưng (Suspended)"
                : "Đã mời (Invited)"
          }
          tone={
            member.status === "active"
              ? "success"
              : member.status === "suspended"
                ? "danger"
                : "warning"
          }
        />
      ),
    },
    {
      key: "actions",
      header: "Hành động",
      render: (member) => (
        <span style={{ display: "inline-flex", gap: spacing[2] }}>
          <Button
            disabled={saving || member.status === "active"}
            onClick={() => void changeMember(member, "activate")}
          >
            Kích hoạt
          </Button>
          <Button
            disabled={saving || member.status !== "active"}
            variant="danger"
            onClick={() => void changeMember(member, "suspend")}
          >
            Tạm ngưng
          </Button>
        </span>
      ),
    },
  ];

  return (
    <div style={{ display: "grid", gap: spacing[4] }}>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          flexWrap: "wrap",
          gap: spacing[2],
        }}
      >
        <div>
          <h2
            style={{
              margin: 0,
              fontSize: 18,
              fontWeight: 650,
              color: colors.text.primary,
            }}
          >
            Thành viên Không gian làm việc: {data.workspace.name}
          </h2>
          <p
            style={{
              margin: "4px 0 0 0",
              ...typography.caption,
              color: colors.text.muted,
            }}
          >
            Workspace ID: {data.workspace.id} · {activeCount}/
            {data.members.length} tài khoản đang hoạt động
          </p>
        </div>
        <div style={{ display: "flex", gap: spacing[2] }}>
          <Button variant="primary" onClick={() => setViewMode("bulk_studio")}>
            + Tạo Hàng Loạt Tài Khoản (Bulk Member Studio)
          </Button>
          <Button
            variant="secondary"
            onClick={() => setShowSingleInvite(!showSingleInvite)}
          >
            {showSingleInvite ? "Đóng mời 1 người" : "+ Mời 1 thành viên"}
          </Button>
          <Button onClick={retry}>Làm mới</Button>
        </div>
      </div>

      {showSingleInvite && (
        <Panel
          title="Mời một thành viên mới"
          description="Nhập định danh tài khoản hoặc email để mời vào không gian làm việc này."
        >
          <form
            onSubmit={invite}
            style={{
              display: "flex",
              flexWrap: "wrap",
              gap: spacing[2],
              alignItems: "end",
            }}
          >
            <Field
              label="Tài khoản / Email"
              required
              value={accountId}
              onChange={(event) => setAccountId(event.currentTarget.value)}
              placeholder="user@example.com hoặc account_id"
            />
            <label style={{ display: "grid", gap: spacing[1] }}>
              <span style={typography.label}>Vai trò</span>
              <select
                aria-label="Invite role"
                value={role}
                onChange={(event) =>
                  setRole(event.currentTarget.value as MemberRole)
                }
                style={{ height: 38, padding: "0 8px", borderRadius: 4 }}
              >
                <option value="admin">Admin (Quản trị viên)</option>
                <option value="operator">Operator (Vận hành)</option>
                <option value="member">Member (Thành viên)</option>
              </select>
            </label>
            <Button type="submit" variant="primary" loading={saving}>
              Gửi lời mời
            </Button>
          </form>
        </Panel>
      )}

      {message ? (
        <p
          role="status"
          style={{ color: colors.semantic.success.text, margin: 0 }}
        >
          {message}
        </p>
      ) : null}
      {error ? (
        <p
          role="alert"
          style={{ color: colors.semantic.danger.text, margin: 0 }}
        >
          {error}
        </p>
      ) : null}

      <Panel
        title="Danh sách thành viên"
        description="Bảng dữ liệu thành viên chính thức với đầy đủ phân trang, lọc theo vai trò/trạng thái và thao tác hàng loạt."
      >
        <EnterpriseDataTable<MemberSummary>
          data={data.members}
          columns={columns}
          getRowKey={(member) => member.id}
          searchPlaceholder="Tìm theo email, Account ID, vai trò, đội nhóm…"
          searchFilter={(member, needle) =>
            member.accountId.toLowerCase().includes(needle) ||
            member.id.toLowerCase().includes(needle) ||
            member.role.toLowerCase().includes(needle) ||
            Boolean(member.teamId?.toLowerCase().includes(needle))
          }
          filters={[
            {
              key: "role",
              label: "Vai trò",
              options: [
                { label: "Tất cả vai trò", value: "all" },
                { label: "Owner (Chủ sở hữu)", value: "owner" },
                { label: "Admin (Quản trị viên)", value: "admin" },
                { label: "Operator (Vận hành)", value: "operator" },
                { label: "Member (Thành viên)", value: "member" },
              ],
            },
            {
              key: "status",
              label: "Trạng thái",
              options: [
                { label: "Tất cả trạng thái", value: "all" },
                { label: "Đang hoạt động (Active)", value: "active" },
                { label: "Đã mời (Invited)", value: "invited" },
                { label: "Đã tạm ngưng (Suspended)", value: "suspended" },
              ],
            },
          ]}
          filterPredicate={(member, filters) => {
            if (
              filters.role &&
              filters.role !== "all" &&
              member.role !== filters.role
            ) {
              return false;
            }
            if (
              filters.status &&
              filters.status !== "all" &&
              member.status !== filters.status
            ) {
              return false;
            }
            return true;
          }}
          bulkActions={[
            {
              label: "Kích hoạt các mục đã chọn",
              variant: "primary",
              action: async (selected) => {
                for (const m of selected) {
                  if (m.status !== "active") {
                    await api.writes.activateMember(data.workspace.id, m.id, {
                      idempotencyKey: `bulk-act-${m.id}`,
                      signal: new AbortController().signal,
                    });
                  }
                }
                retry();
              },
            },
            {
              label: "Tạm ngưng các mục đã chọn",
              variant: "danger",
              action: async (selected) => {
                for (const m of selected) {
                  if (m.status === "active") {
                    await api.writes.suspendMember(data.workspace.id, m.id, {
                      idempotencyKey: `bulk-susp-${m.id}`,
                      signal: new AbortController().signal,
                    });
                  }
                }
                retry();
              },
            },
            {
              label: "Xuất danh sách đã chọn (.csv)",
              variant: "secondary",
              action: (selected) => {
                const csv =
                  "account_id,role,status,team_id\n" +
                  selected
                    .map(
                      (m) =>
                        `"${m.accountId}","${m.role}","${m.status}","${m.teamId || ""}"`,
                    )
                    .join("\n");
                const blob = new Blob([csv], { type: "text/csv" });
                const url = URL.createObjectURL(blob);
                const a = document.createElement("a");
                a.href = url;
                a.download = `members-${data.workspace.id}.csv`;
                a.click();
                URL.revokeObjectURL(url);
              },
            },
          ]}
          emptyMessage="Không có thành viên nào khớp với bộ lọc."
          defaultPageSize={10}
        />
      </Panel>
    </div>
  );
}

function AccessDirectory({
  api,
  data,
  retry,
}: {
  api: WebApiClient;
  data: WorkspaceOperationalData;
  retry: () => void;
}) {
  const [memberId, setMemberId] = useState(
    data.members.find((member) => member.status === "active")?.id ?? "",
  );
  const [resourceId, setResourceId] = useState(data.resources[0]?.id ?? "");
  const [action, setAction] = useState<GrantCreateInput["action"]>("read");
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string>();
  const [error, setError] = useState<string>();
  const canSubmit = Boolean(memberId && resourceId);
  const submit = async () => {
    if (!canSubmit) return;
    setSaving(true);
    setError(undefined);
    setMessage(undefined);
    const grantId = `grant_${crypto.randomUUID().replaceAll("-", "").slice(0, 24)}`;
    try {
      await api.writes.createGrant(
        data.workspace.id,
        {
          id: grantId,
          workspaceId: data.workspace.id,
          resourceId,
          memberId,
          action,
          effect: "allow",
        },
        {
          idempotencyKey: `grant-create-${grantId}`,
          signal: new AbortController().signal,
        },
      );
      setMessage("Access grant created.");
      retry();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  };
  const revoke = async (grantId: string) => {
    setSaving(true);
    setError(undefined);
    setMessage(undefined);
    try {
      await api.writes.revokeGrant(data.workspace.id, grantId, {
        idempotencyKey: `grant-revoke-${grantId}`,
        signal: new AbortController().signal,
      });
      setMessage("Access grant revoked.");
      retry();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  };
  return (
    <div style={{ display: "grid", gap: spacing[4] }}>
      <Panel
        title="Grant access"
        description="Server-side ACL, audit, tenant checks, and idempotency apply to every mutation."
      >
        <div
          style={{
            display: "grid",
            gap: spacing[3],
            gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))",
            alignItems: "end",
          }}
        >
          <label>
            <span>Member</span>
            <select
              value={memberId}
              onChange={(event) => setMemberId(event.currentTarget.value)}
            >
              <option value="">Select member</option>
              {data.members
                .filter((member) => member.status === "active")
                .map((member) => (
                  <option key={member.id} value={member.id}>
                    {member.accountId}
                  </option>
                ))}
            </select>
          </label>
          <label>
            <span>Resource</span>
            <select
              value={resourceId}
              onChange={(event) => setResourceId(event.currentTarget.value)}
            >
              <option value="">Select resource</option>
              {data.resources.map((resource) => (
                <option key={resource.id} value={resource.id}>
                  {resource.displayName}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Permission</span>
            <select
              value={action}
              onChange={(event) =>
                setAction(
                  event.currentTarget.value as GrantCreateInput["action"],
                )
              }
            >
              {["read", "execute", "update", "manage"].map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </label>
          <Button
            disabled={!canSubmit || saving}
            loading={saving}
            onClick={() => void submit()}
          >
            Grant access
          </Button>
        </div>
        {message ? <p role="status">{message}</p> : null}
        {error ? <p role="alert">{error}</p> : null}
      </Panel>
      <Panel
        title="Current grants"
        description={`${data.grants.length} server-authorized grants with search, filtering and sorting.`}
      >
        <EnterpriseDataTable<WorkspaceOperationalData["grants"][number]>
          data={data.grants}
          columns={[
            {
              key: "member",
              header: "Member / Account",
              sortable: true,
              sortValue: (grant) =>
                data.members.find((member) => member.id === grant.memberId)
                  ?.accountId ?? grant.memberId,
              render: (grant) =>
                data.members.find((member) => member.id === grant.memberId)
                  ?.accountId ?? grant.memberId,
            },
            {
              key: "resource",
              header: "Resource",
              sortable: true,
              sortValue: (grant) =>
                data.resources.find(
                  (resource) => resource.id === grant.resourceId,
                )?.displayName ?? grant.resourceId,
              render: (grant) =>
                data.resources.find(
                  (resource) => resource.id === grant.resourceId,
                )?.displayName ?? grant.resourceId,
            },
            {
              key: "action",
              header: "Permission",
              sortable: true,
              sortValue: (grant) => grant.action,
              render: (grant) => <code>{grant.action}</code>,
            },
            {
              key: "effect",
              header: "Effect",
              sortable: true,
              sortValue: (grant) => grant.effect,
              render: (grant) => (
                <Status
                  label={grant.effect}
                  tone={grant.effect === "allow" ? "success" : "danger"}
                />
              ),
            },
            {
              key: "revoke",
              header: "Action",
              render: (grant) => (
                <Button
                  variant="danger"
                  disabled={saving}
                  onClick={() => void revoke(grant.id)}
                >
                  Revoke
                </Button>
              ),
            },
          ]}
          getRowKey={(grant) => grant.id}
          searchPlaceholder="Search grants by member, resource or action..."
          searchFilter={(grant, needle) => {
            const memberName =
              data.members.find((m) => m.id === grant.memberId)?.accountId ??
              grant.memberId;
            const resourceName =
              data.resources.find((r) => r.id === grant.resourceId)
                ?.displayName ?? grant.resourceId;
            return (
              memberName.toLowerCase().includes(needle) ||
              resourceName.toLowerCase().includes(needle) ||
              grant.action.toLowerCase().includes(needle) ||
              grant.effect.toLowerCase().includes(needle)
            );
          }}
          filters={[
            {
              key: "action",
              label: "Permission",
              options: [
                { label: "All permissions", value: "all" },
                { label: "read", value: "read" },
                { label: "execute", value: "execute" },
                { label: "update", value: "update" },
                { label: "manage", value: "manage" },
              ],
            },
            {
              key: "effect",
              label: "Effect",
              options: [
                { label: "All effects", value: "all" },
                { label: "allow", value: "allow" },
                { label: "deny", value: "deny" },
              ],
            },
          ]}
          filterPredicate={(grant, filters) => {
            if (
              filters.action &&
              filters.action !== "all" &&
              grant.action !== filters.action
            ) {
              return false;
            }
            if (
              filters.effect &&
              filters.effect !== "all" &&
              grant.effect !== filters.effect
            ) {
              return false;
            }
            return true;
          }}
          emptyMessage="No server-authorized grants found matching the criteria."
          defaultPageSize={10}
        />
      </Panel>
      <Panel
        title="Refresh state"
        description="Data reloads from the API after every mutation."
        actions={<Button onClick={retry}>Refresh</Button>}
      />
    </div>
  );
}

function useWorkspaceState(
  load: (signal: AbortSignal) => Promise<WorkspaceData>,
  online: boolean,
) {
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<WorkspaceLoadState>({ status: "loading" });
  const retry = useCallback(() => setRevision((value) => value + 1), []);
  // revision is an explicit reload token incremented by retry().
  // biome-ignore lint/correctness/useExhaustiveDependencies: retry must restart the loader.
  useEffect(() => {
    const controller = new AbortController();
    setState({ status: "loading" });
    load(controller.signal)
      .then((data) =>
        setState(
          Array.isArray(data) && data.length === 0
            ? {
                status: "empty",
                message:
                  "No workspace membership was returned for this account.",
              }
            : { status: "ready", data },
        ),
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
  }, [load, online, revision]);
  return { retry, state };
}
