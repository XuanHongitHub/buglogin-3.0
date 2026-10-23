import { useCallback, useState } from "react";
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
  errorMessage,
  type Operation,
  type Profile,
  type ProfileCreateInput,
  type ProfileCreateRuntime,
  type ProxyParseReport,
  type VersionedProfile,
  type VersionedProfilePage,
  type WebApiClient,
  WebApiError,
} from "../api";
import {
  AsyncState,
  pageHeaderStyle,
  useLoadState,
  useOnline,
  WebShell,
} from "../shells";

const runtimeOptions: ProfileCreateRuntime[] = ["wayfern"];

export function OperationsPage({
  api,
  path,
}: {
  api: WebApiClient;
  path: string;
}) {
  const online = useOnline();
  const [cursorHistory, setCursorHistory] = useState<(string | null)[]>([null]);
  const cursor = cursorHistory[cursorHistory.length - 1] ?? null;
  const load = useCallback(
    (signal: AbortSignal) => api.reads.profiles(cursor, 50, signal),
    [api, cursor],
  );
  const { state, retry } = useLoadState(
    load,
    (page) => page.items.length === 0,
    "No profiles are available for this account.",
  );
  const resetProfiles = () => {
    setCursorHistory([null]);
    retry();
  };
  const nextProfiles = (nextCursor: string | null) => {
    if (nextCursor) setCursorHistory((current) => [...current, nextCursor]);
  };
  const previousProfiles = () =>
    setCursorHistory((current) =>
      current.length > 1 ? current.slice(0, -1) : current,
    );
  return (
    <WebShell
      activePath={path}
      mode="operations"
      nav={[
        { href: "/operations", label: "Profile operations" },
        { href: "/workspaces", label: "Workspaces" },
        { href: "/account/profile", label: "Account" },
      ]}
      title="Profile operations"
    >
      <h1 style={pageHeaderStyle}>Profile operations</h1>
      {!online ? (
        <Panel
          title="Remote operations unavailable offline"
          description="Reconnect before creating, launching, stopping, or parsing proxies."
        />
      ) : null}
      <div style={{ display: "grid", gap: spacing[4] }}>
        <ProfileCreateForm
          api={api}
          disabled={!online}
          onCreated={resetProfiles}
        />
        <ProxyParser api={api} disabled={!online} />
        <AsyncState retry={retry} state={state}>
          {(page) => (
            <ProfileInventory
              api={api}
              disabled={!online}
              page={page}
              retry={retry}
              canGoBack={cursorHistory.length > 1}
              onNext={nextProfiles}
              onPrevious={previousProfiles}
            />
          )}
        </AsyncState>
      </div>
    </WebShell>
  );
}

function ProfileCreateForm({
  api,
  disabled,
  onCreated,
}: {
  api: WebApiClient;
  disabled: boolean;
  onCreated: () => void;
}) {
  const [id, setId] = useState(
    () => `prf_${crypto.randomUUID().replaceAll("-", "").slice(0, 20)}`,
  );
  const [name, setName] = useState("");
  const [runtime, setRuntime] = useState<ProfileCreateRuntime>("wayfern");
  const [runtimeVersion, setRuntimeVersion] = useState("");
  const [note, setNote] = useState("");
  const [state, setState] = useState<"idle" | "loading" | "success" | "error">(
    "idle",
  );
  const [message, setMessage] = useState("");
  const idError =
    id.length === 0 ||
    id.length > 128 ||
    !/^[A-Za-z0-9][A-Za-z0-9._:-]*$/.test(id)
      ? "Use 1–128 ASCII characters; start with a letter or number."
      : undefined;
  const canSubmit =
    !disabled && !idError && name.trim().length > 0 && state !== "loading";
  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!canSubmit) return;
    setState("loading");
    setMessage("");
    const profile: ProfileCreateInput = {
      id,
      name: name.trim(),
      runtime,
      ...(runtimeVersion.trim()
        ? { runtimeVersion: runtimeVersion.trim() }
        : {}),
      ...(note.trim() ? { note: note.trim() } : {}),
    };
    try {
      await api.writes.createProfile(profile, {
        idempotencyKey: `profile-create-${id}`,
        signal: new AbortController().signal,
      });
      setState("success");
      setMessage("Profile created by the API.");
      setName("");
      setNote("");
      setId(`prf_${crypto.randomUUID().replaceAll("-", "").slice(0, 20)}`);
      onCreated();
    } catch (error) {
      setState("error");
      setMessage(errorMessage(error));
    }
  };
  return (
    <Panel
      title="Create profile"
      description="Creates a real profile record using the sync policy enforced by the workspace API; no local mock state is used."
    >
      <form onSubmit={submit} style={{ display: "grid", gap: spacing[3] }}>
        <div
          style={{
            display: "grid",
            gap: spacing[3],
            gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
          }}
        >
          <Field
            error={idError}
            help="Canonical safe ID; immutable after creation."
            label="Profile ID"
            onChange={(event) => setId(event.target.value)}
            required
            value={id}
          />
          <Field
            label="Name"
            onChange={(event) => setName(event.target.value)}
            required
            value={name}
          />
          <label style={{ display: "grid", gap: spacing[1] }}>
            <span style={typography.label}>Runtime</span>
            <select
              aria-label="Runtime"
              onChange={(event) =>
                setRuntime(event.target.value as ProfileCreateRuntime)
              }
              value={runtime}
              style={controlStyle}
            >
              {runtimeOptions.map((option) => (
                <option key={option} value={option}>
                  {option}
                </option>
              ))}
            </select>
          </label>
          <Field
            help="Optional runtime compatibility version."
            label="Runtime version"
            onChange={(event) => setRuntimeVersion(event.target.value)}
            value={runtimeVersion}
          />
        </div>
        <Field
          label="Note"
          multiline
          onChange={(event) => setNote(event.target.value)}
          value={note}
        />
        {state === "success" ? (
          <Status label={message} tone="success" live />
        ) : null}
        {state === "error" ? (
          <Status label={message} tone="danger" live />
        ) : null}
        <div>
          <Button
            disabled={!canSubmit}
            loading={state === "loading"}
            type="submit"
            variant="primary"
          >
            Create profile
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function ProfileInventory({
  api,
  disabled,
  page,
  retry,
  canGoBack,
  onNext,
  onPrevious,
}: {
  api: WebApiClient;
  disabled: boolean;
  page: VersionedProfilePage;
  retry: () => void;
  canGoBack: boolean;
  onNext: (cursor: string | null) => void;
  onPrevious: () => void;
}) {
  const [profilesRoot, setProfilesRoot] = useState("");
  const [operation, setOperation] = useState<Record<string, OperationState>>(
    {},
  );
  const run = async (profile: Profile, action: "launch" | "stop") => {
    if (disabled || operation[profile.id]?.status === "loading") return;
    if (action === "launch" && !profilesRoot.trim()) {
      setOperation((current) => ({
        ...current,
        [profile.id]: {
          status: "error",
          message: "Enter the local profiles root before launching.",
        },
      }));
      return;
    }
    setOperation((current) => ({
      ...current,
      [profile.id]: { status: "loading" },
    }));
    try {
      const options = {
        idempotencyKey: `${action}-profile-${profile.id}-${Date.now()}`,
        signal: new AbortController().signal,
      };
      const initial =
        action === "launch"
          ? await api.writes.launchProfile(
              profile.id,
              { profilesRoot: profilesRoot.trim(), extraArgs: [] },
              options,
            )
          : await api.writes.stopProfile(profile.id, options);
      const final =
        initial.status === "queued" || initial.status === "running"
          ? await pollOperation(api, initial)
          : initial;
      if (final.status === "failed" || final.status === "cancelled")
        throw new WebApiError(
          final.error ?? {
            type: "operation",
            title: "Operation failed",
            status: 502,
            code: "operation_failed",
            correlationId: "operation",
            retryable: false,
            fields: {},
          },
        );
      setOperation((current) => ({
        ...current,
        [profile.id]: {
          status: "success",
          message: `${action === "launch" ? "Launch" : "Stop"} completed.`,
        },
      }));
      retry();
    } catch (error) {
      setOperation((current) => ({
        ...current,
        [profile.id]: { status: "error", message: errorMessage(error) },
      }));
    }
  };
  const columns: TableColumn<VersionedProfile>[] = [
    {
      key: "profile",
      header: "Profile",
      render: (row) => (
        <div>
          <strong>{row.value.name}</strong>
          <div style={metaStyle}>{row.value.id}</div>
        </div>
      ),
    },
    {
      key: "runtime",
      header: "Runtime",
      render: (row) => runtimeLabel(row.value),
    },
    {
      key: "sync",
      header: "Sync",
      render: (row) => row.value.syncMode ?? "unset",
    },
    {
      key: "state",
      header: "State",
      render: (row) => (
        <Status
          label={row.value.state}
          tone={
            row.value.state === "running"
              ? "success"
              : row.value.state === "failed"
                ? "danger"
                : "neutral"
          }
        />
      ),
    },
    {
      key: "actions",
      header: "Actions",
      render: (row) => (
        <div style={{ display: "flex", flexWrap: "wrap", gap: spacing[2] }}>
          <Button
            disabled={
              disabled ||
              row.value.archived ||
              row.value.state !== "ready" ||
              row.value.runtime === "preserved"
            }
            loading={operation[row.value.id]?.status === "loading"}
            onClick={() => void run(row.value, "launch")}
            variant="primary"
          >
            Launch
          </Button>
          <Button
            disabled={
              disabled || row.value.archived || row.value.state !== "running"
            }
            loading={operation[row.value.id]?.status === "loading"}
            onClick={() => void run(row.value, "stop")}
          >
            Stop
          </Button>
        </div>
      ),
    },
  ];
  return (
    <Panel
      title="Profiles"
      description="Live records returned by `/v1/profiles`. Launch and stop use backend operations and refresh after completion."
    >
      <div
        style={{
          display: "grid",
          gap: spacing[3],
          marginBottom: spacing[4],
          maxWidth: 560,
        }}
      >
        <Field
          help="Required by the local runtime launch endpoint. Never sent for stop."
          label="Local profiles root"
          onChange={(event) => setProfilesRoot(event.target.value)}
          value={profilesRoot}
        />
      </div>
      <Table
        caption="Profiles"
        columns={columns}
        emptyMessage="No profiles are available for this account."
        getRowKey={(row) => row.value.id}
        rows={page.items}
      />
      {Object.entries(operation).map(([profileId, value]) =>
        value.status === "success" ? (
          <div
            key={profileId}
            aria-live="polite"
            style={{ marginTop: spacing[2] }}
          >
            <Status
              label={`${profileId}: ${value.message}`}
              tone="success"
              live
            />
          </div>
        ) : value.status === "error" ? (
          <div
            key={profileId}
            aria-live="polite"
            style={{ marginTop: spacing[2] }}
          >
            <Status
              label={`${profileId}: ${value.message}`}
              tone="danger"
              live
            />
          </div>
        ) : null,
      )}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: spacing[2],
          marginTop: spacing[3],
        }}
      >
        <span>{page.items.length} profiles on this page</span>
        <div style={{ display: "flex", gap: spacing[2] }}>
          <Button disabled={!canGoBack} onClick={onPrevious}>
            Previous
          </Button>
          <Button
            disabled={!page.nextCursor}
            onClick={() => onNext(page.nextCursor)}
          >
            Next
          </Button>
          <Button onClick={retry}>Refresh</Button>
        </div>
      </div>
    </Panel>
  );
}

function runtimeLabel(profile: Profile): string {
  if (profile.runtime !== "preserved") return profile.runtime;
  if (
    typeof profile.runtimeConfig === "object" &&
    profile.runtimeConfig !== null &&
    !Array.isArray(profile.runtimeConfig)
  ) {
    const migration = profile.runtimeConfig.migration;
    if (
      typeof migration === "object" &&
      migration !== null &&
      !Array.isArray(migration) &&
      typeof migration.legacyRuntime === "string"
    ) {
      return `${migration.legacyRuntime} (preserved)`;
    }
  }
  return "Legacy runtime (preserved)";
}

function ProxyParser({
  api,
  disabled,
}: {
  api: WebApiClient;
  disabled: boolean;
}) {
  const [text, setText] = useState("");
  const [state, setState] = useState<{
    status: "idle" | "loading" | "error";
    message?: string;
    report?: ProxyParseReport;
  }>({ status: "idle" });
  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (disabled || !text.trim()) return;
    setState({ status: "loading" });
    try {
      setState({
        status: "idle",
        report: await api.writes.parseProxies(
          text,
          new AbortController().signal,
        ),
      });
    } catch (error) {
      setState({ status: "error", message: errorMessage(error) });
    }
  };
  const report = state.report;
  return (
    <Panel
      title="Fast proxy parse"
      description="Parses the textarea through `/v1/proxies/parse`; credentials are not displayed by this view."
    >
      <form onSubmit={submit} style={{ display: "grid", gap: spacing[3] }}>
        <Field
          help="One proxy per line. The server returns valid, invalid, and duplicate counts."
          label="Proxy input"
          multiline
          onChange={(event) => setText(event.target.value)}
          value={text}
        />
        <div>
          <Button
            disabled={disabled || !text.trim()}
            loading={state.status === "loading"}
            type="submit"
            variant="primary"
          >
            Parse proxies
          </Button>
        </div>
      </form>
      {state.status === "error" ? (
        <Status
          label={state.message ?? "Proxy parse failed."}
          tone="danger"
          live
        />
      ) : null}
      {report ? <ProxyReport report={report} /> : null}
    </Panel>
  );
}

function ProxyReport({ report }: { report: ProxyParseReport }) {
  const columns: TableColumn<ProxyParseReport["rows"][number]>[] = [
    { key: "line", header: "Line", render: (row) => row.line ?? "—" },
    {
      key: "status",
      header: "Status",
      render: (row) => (
        <Status
          label={row.valid ? "Valid" : "Invalid"}
          tone={row.valid ? "success" : "danger"}
        />
      ),
    },
    {
      key: "protocol",
      header: "Protocol",
      render: (row) => row.protocol ?? "—",
    },
    {
      key: "endpoint",
      header: "Endpoint",
      render: (row) => row.endpoint ?? row.error ?? "—",
    },
  ];
  return (
    <div style={{ display: "grid", gap: spacing[3], marginTop: spacing[4] }}>
      <div style={{ display: "flex", flexWrap: "wrap", gap: spacing[2] }}>
        <Status label={`${report.validCount} valid`} tone="success" />
        <Status
          label={`${report.invalidCount} invalid`}
          tone={report.invalidCount ? "danger" : "neutral"}
        />
        <Status
          label={`${report.duplicateCount} duplicate`}
          tone={report.duplicateCount ? "warning" : "neutral"}
        />
      </div>
      <Table
        caption="Proxy parse results"
        columns={columns}
        emptyMessage="The parser returned no rows."
        getRowKey={(row, index) => `${row.line ?? "valid"}-${index}`}
        rows={report.rows}
      />
    </div>
  );
}

type OperationState =
  | { status: "loading" }
  | { status: "success" | "error"; message: string };

async function pollOperation(
  api: WebApiClient,
  initial: Operation,
): Promise<Operation> {
  let current = initial;
  for (
    let attempt = 0;
    attempt < 60 &&
    (current.status === "queued" || current.status === "running");
    attempt += 1
  ) {
    await new Promise((resolve) => setTimeout(resolve, 500));
    current = await api.reads.operation(
      current.operationId,
      new AbortController().signal,
    );
  }
  if (current.status === "queued" || current.status === "running")
    throw new Error(
      "Operation polling timed out. Refresh and inspect the profile state.",
    );
  return current;
}

const controlStyle = {
  background: colors.surface.default,
  border: `1px solid ${colors.border.default}`,
  borderRadius: 5,
  boxSizing: "border-box" as const,
  color: colors.text.primary,
  fontFamily: typography.family,
  fontSize: 14,
  minHeight: 34,
  padding: `${spacing[1.5]}px ${spacing[2]}px`,
  width: "100%",
};
const metaStyle = { color: colors.text.muted, ...typography.caption };
