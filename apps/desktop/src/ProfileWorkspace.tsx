import {
  type FormEvent,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Button,
  Field,
  Panel,
  Status,
} from "../../../frontend/src/design-system/index.js";
import {
  createTauriDesktopApi,
  type DesktopApi,
  normalizeProblem,
} from "./bridge.js";
import type {
  Problem,
  Profile,
  ProfileCreateInput,
  ProfileState,
  ProxyCreateInput,
  ProxyMetadataInput,
  ProxyParseReport,
  ProxyProtocol,
  ProxyRecord,
  ProxyUpdateInput,
  Workspace,
} from "./contracts.js";
import {
  canLaunch,
  canStop,
  displayedProfiles,
  failedProfiles,
  type LoadState,
  loadedProfiles,
  type ProfileAction,
  projectedState,
} from "./model.js";
import { normalizedRequiredText, usableRevision } from "./profile-workflow.js";
import "./desktop.css";

export type ProfileWorkspaceProps = { api?: DesktopApi };
type ProfileFilter = "all" | "running" | "failed" | "archived";
type Job = {
  id: string;
  profile: string;
  kind: "launch" | "stop" | "archive" | "restore";
  status: "queued" | "running" | "succeeded" | "failed";
  detail: string;
};

type IconName =
  | "profiles"
  | "proxy"
  | "team"
  | "resource"
  | "settings"
  | "search";
function Icon({ name }: { name: IconName }) {
  const paths: Record<IconName, string> = {
    profiles: "M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z",
    proxy:
      "M8.5 8.5l7 7M7 17l-2 2a3 3 0 0 1-4-4l4-4a3 3 0 0 1 4 0M17 7l2-2a3 3 0 0 0-4-4l-4 4a3 3 0 0 0 0 4",
    team: "M8 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8Zm8-1a3 3 0 1 0 0-6m-1 12c0-3-3-5-7-5s-7 2-7 5m13-4c3 .3 5 1.8 5 4",
    resource: "M5 5h14v14H5zM9 9h6v6H9z",
    settings:
      "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm0-6v3m0 14v3M4.2 4.2l2.1 2.1m11.4 11.4 2.1 2.1M2 12h3m14 0h3M4.2 19.8l2.1-2.1m11.4-11.4 2.1-2.1",
    search: "m21 21-4.3-4.3M10.8 18a7.2 7.2 0 1 1 0-14.4 7.2 7.2 0 0 1 0 14.4",
  };
  return (
    <svg className="icon" viewBox="0 0 24 24" aria-hidden="true">
      <path d={paths[name]} />
    </svg>
  );
}

export function ProfileWorkspace({ api: providedApi }: ProfileWorkspaceProps) {
  const api = useMemo(
    () => providedApi ?? createTauriDesktopApi(),
    [providedApi],
  );
  const [loadState, setLoadState] = useState<LoadState>({ kind: "loading" });
  const loadStateRef = useRef(loadState);
  const [createOpen, setCreateOpen] = useState(false);
  const [proxyOpen, setProxyOpen] = useState(false);
  const [cloneTarget, setCloneTarget] = useState<Profile>();
  const [shareTarget, setShareTarget] = useState<Profile>();
  const [revokeTarget, setRevokeTarget] = useState<Profile>();
  const [syncRecoveryTarget, setSyncRecoveryTarget] = useState<Profile>();
  const [filter, setFilter] = useState<ProfileFilter>("all");
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string>();
  const [actions, setActions] = useState<Record<string, ProfileAction>>({});
  const [mutatingProfiles, setMutatingProfiles] = useState<
    Record<string, Problem | true>
  >({});
  const [jobs, setJobs] = useState<Job[]>([]);
  const [workspaceLabel, setWorkspaceLabel] = useState("Workspace");
  const profiles = displayedProfiles(loadState);

  useEffect(() => {
    loadStateRef.current = loadState;
  }, [loadState]);

  const refresh = useCallback(async () => {
    const cached = displayedProfiles(loadStateRef.current);
    setLoadState({ kind: "loading" });
    try {
      const next = loadedProfiles((await api.listProfiles(true)).items);
      setLoadState(next);
      if (next.kind === "ready")
        setSelectedId((current) => current ?? next.profiles[0]?.id);
    } catch (error) {
      setLoadState(failedProfiles(normalizeProblem(error), cached));
    }
  }, [api]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const organizations = await api.listOrganizations();
        const pages = await Promise.all(
          organizations.items.map((organization) =>
            api.listWorkspaces(organization.id),
          ),
        );
        const workspace = pages.flatMap((page) => page.items)[0];
        if (active && workspace?.name) setWorkspaceLabel(workspace.name);
      } catch {
        if (active) setWorkspaceLabel("Workspace");
      }
    })();
    return () => {
      active = false;
    };
  }, [api]);

  const visibleProfiles = useMemo(
    () =>
      profiles.filter((profile) => {
        const state = projectedState(profile, actions[profile.id]);
        const matchesFilter =
          filter === "all"
            ? !profile.archived
            : filter === "running"
              ? !profile.archived && state === "running"
              : filter === "failed"
                ? !profile.archived && state === "failed"
                : Boolean(profile.archived);
        const text =
          `${profile.name} ${profile.id} ${profile.runtime}`.toLowerCase();
        return matchesFilter && text.includes(query.trim().toLowerCase());
      }),
    [actions, filter, profiles, query],
  );
  const selected =
    profiles.find((profile) => profile.id === selectedId) ?? visibleProfiles[0];
  const runningCount = profiles.filter(
    (profile) => projectedState(profile, actions[profile.id]) === "running",
  ).length;
  const reviewCount = profiles.filter(
    (profile) =>
      !profile.archived &&
      projectedState(profile, actions[profile.id]) === "failed",
  ).length;
  const archivedCount = profiles.filter((profile) => profile.archived).length;

  const runAction = async (profile: Profile, kind: "launch" | "stop") => {
    const jobId = `job_${crypto.randomUUID()}`;
    setActions((current) => ({
      ...current,
      [profile.id]: { profileId: profile.id, kind },
    }));
    setJobs((current) =>
      [
        {
          id: jobId,
          profile: profile.name,
          kind,
          status: "queued" as const,
          detail: "Waiting for authority.",
        },
        ...current,
      ].slice(0, 6),
    );
    try {
      setJobs((current) =>
        current.map((job) =>
          job.id === jobId
            ? {
                ...job,
                status: "running" as const,
                detail: "Runtime handshake in progress.",
              }
            : job,
        ),
      );
      const accepted =
        kind === "launch"
          ? await api.launchProfile(profile.id)
          : await api.stopProfile(profile.id);
      setActions((current) => ({
        ...current,
        [profile.id]: {
          profileId: profile.id,
          kind,
          operationId: accepted.operationId,
        },
      }));
      await waitForOperation(api, accepted.operationId);
      setJobs((current) =>
        current.map((job) =>
          job.id === jobId
            ? {
                ...job,
                status: "succeeded" as const,
                detail: "Authority confirmed.",
              }
            : job,
        ),
      );
      setActions((current) => {
        const next = { ...current };
        delete next[profile.id];
        return next;
      });
      await refresh();
    } catch (error) {
      const problem = normalizeProblem(error);
      setJobs((current) =>
        current.map((job) =>
          job.id === jobId
            ? {
                ...job,
                status: "failed" as const,
                detail: problem.detail ?? problem.title,
              }
            : job,
        ),
      );
      setActions((current) => ({
        ...current,
        [profile.id]: { profileId: profile.id, kind, problem },
      }));
    }
  };

  const mutateArchive = async (profile: Profile) => {
    if (profile.revision === undefined) {
      setMutatingProfiles((current) => ({
        ...current,
        [profile.id]: fieldProblem(
          "Profile revision is unavailable. Refresh before retrying.",
        ),
      }));
      return;
    }
    const kind: Job["kind"] = profile.archived ? "restore" : "archive";
    const jobId = `job_${crypto.randomUUID()}`;
    setMutatingProfiles((current) => ({ ...current, [profile.id]: true }));
    setJobs((current) =>
      [
        {
          id: jobId,
          profile: profile.name,
          kind,
          status: "running" as const,
          detail: `${kind === "archive" ? "Archiving" : "Restoring"} profile.`,
        },
        ...current,
      ].slice(0, 6),
    );
    try {
      if (kind === "archive")
        await api.archiveProfile(profile.id, profile.revision);
      else await api.restoreProfile(profile.id, profile.revision);
      setJobs((current) =>
        current.map((job) =>
          job.id === jobId
            ? {
                ...job,
                status: "succeeded" as const,
                detail: "Authority confirmed.",
              }
            : job,
        ),
      );
      setMutatingProfiles((current) => {
        const next = { ...current };
        delete next[profile.id];
        return next;
      });
      await refresh();
    } catch (error) {
      const problem = normalizeProblem(error);
      setJobs((current) =>
        current.map((job) =>
          job.id === jobId
            ? {
                ...job,
                status: "failed" as const,
                detail: problem.detail ?? problem.title,
              }
            : job,
        ),
      );
      setMutatingProfiles((current) => ({ ...current, [profile.id]: problem }));
    }
  };

  return (
    <div className="desktop-shell">
      <aside className="desktop-rail" aria-label="Application navigation">
        <div className="desktop-brand" aria-hidden="true">
          BL
        </div>
        <button
          className="rail-button rail-button-active"
          type="button"
          aria-current="page"
          aria-label="Profiles"
        >
          <Icon name="profiles" />
        </button>
        <button
          className="rail-button"
          type="button"
          aria-label="Proxy library"
          onClick={() => setProxyOpen(true)}
        >
          <Icon name="proxy" />
        </button>
        <div className="rail-spacer" />
        <button className="rail-button" type="button" aria-label="Settings">
          <Icon name="settings" />
        </button>
        <div className="desktop-avatar" aria-hidden="true">
          AN
        </div>
      </aside>
      <main className="desktop-main">
        <header className="desktop-topbar">
          <div className="window-controls" aria-hidden="true">
            <i className="window-dot window-dot-red" />
            <i className="window-dot window-dot-yellow" />
            <i className="window-dot window-dot-green" />
          </div>
          <nav className="profile-tabs" aria-label="Profile groups">
            <button
              className={
                filter === "all"
                  ? "profile-tab profile-tab-active"
                  : "profile-tab"
              }
              type="button"
              onClick={() => setFilter("all")}
            >
              All <span>{profiles.length}</span>
            </button>
            <button
              className={
                filter === "running"
                  ? "profile-tab profile-tab-active"
                  : "profile-tab"
              }
              type="button"
              onClick={() => setFilter("running")}
            >
              Running <span>{runningCount}</span>
            </button>
            <button
              className={
                filter === "failed"
                  ? "profile-tab profile-tab-active"
                  : "profile-tab"
              }
              type="button"
              onClick={() => setFilter("failed")}
            >
              Needs review <span>{reviewCount}</span>
            </button>
            <button
              className={
                filter === "archived"
                  ? "profile-tab profile-tab-active"
                  : "profile-tab"
              }
              type="button"
              onClick={() => setFilter("archived")}
            >
              Archived <span>{archivedCount}</span>
            </button>
          </nav>
          <div className="topbar-actions">
            <ConnectionStatus state={loadState} />
            <label className="profile-search">
              <Icon name="search" />
              <span className="sr-only">Search profiles</span>
              <input
                placeholder="Search profiles…"
                value={query}
                onChange={(event) => setQuery(event.currentTarget.value)}
              />
            </label>
            <Button variant="primary" onClick={() => setCreateOpen(true)}>
              ＋ New
            </Button>
          </div>
        </header>
        <div className="desktop-content">
          <div className="profile-toolbar profile-toolbar-secondary">
            <div className="toolbar-context">
              <strong>{workspaceLabel}</strong>
              <span>
                Workspace ·{" "}
                {loadState.kind === "offline" ? "Local mode" : "Control plane"}
              </span>
            </div>
            <Button onClick={() => setProxyOpen(true)}>Parse proxies</Button>
            <Button onClick={() => void refresh()}>Refresh</Button>
          </div>
          <StateNotice state={loadState} onRetry={refresh} />
          <section
            className="profile-table-section"
            aria-label="Profile inventory"
          >
            <div className="table-scroll">
              <table className="profile-table">
                <caption>Profiles</caption>
                <thead>
                  <tr>
                    <th className="check-column">
                      <input type="checkbox" aria-label="Select all profiles" />
                    </th>
                    <th>Profile</th>
                    <th>Tags</th>
                    <th>Note</th>
                    <th>Proxy / VPN</th>
                    <th>Runtime</th>
                    <th>State</th>
                    <th>
                      <span className="sr-only">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {visibleProfiles.map((profile) => (
                    <ProfileRow
                      key={profile.id}
                      profile={profile}
                      action={actions[profile.id]}
                      selected={profile.id === selected?.id}
                      onSelect={() => setSelectedId(profile.id)}
                      onRun={runAction}
                      onArchive={mutateArchive}
                      onClone={() => setCloneTarget(profile)}
                      onShare={() => setShareTarget(profile)}
                      onRevoke={() => setRevokeTarget(profile)}
                      mutating={mutatingProfiles[profile.id]}
                    />
                  ))}
                </tbody>
              </table>
              {visibleProfiles.length === 0 ? (
                <EmptyState
                  state={loadState}
                  onCreate={() => setCreateOpen(true)}
                />
              ) : null}
            </div>
          </section>
          <div className="desktop-lower-grid">
            <Inspector
              profile={selected}
              action={selected ? actions[selected.id] : undefined}
              onSyncRecovery={() => selected && setSyncRecoveryTarget(selected)}
            />
            <Jobs jobs={jobs} />
          </div>
        </div>
      </main>
      {createOpen ? (
        <CreateDrawer
          api={api}
          onClose={() => setCreateOpen(false)}
          onCreated={async () => {
            setCreateOpen(false);
            await refresh();
          }}
        />
      ) : null}
      {proxyOpen ? (
        <ProxyDrawer api={api} onClose={() => setProxyOpen(false)} />
      ) : null}
      {cloneTarget ? (
        <CloneDrawer
          api={api}
          profile={cloneTarget}
          onClose={() => setCloneTarget(undefined)}
          onCompleted={refresh}
        />
      ) : null}
      {shareTarget ? (
        <ShareDrawer
          api={api}
          profile={shareTarget}
          onClose={() => setShareTarget(undefined)}
          onCompleted={refresh}
        />
      ) : null}
      {revokeTarget ? (
        <RevokeDrawer
          api={api}
          profile={revokeTarget}
          onClose={() => setRevokeTarget(undefined)}
          onCompleted={refresh}
        />
      ) : null}
      {syncRecoveryTarget ? (
        <SyncRecoveryDrawer
          api={api}
          profile={syncRecoveryTarget}
          onClose={() => setSyncRecoveryTarget(undefined)}
        />
      ) : null}
    </div>
  );
}

function ProfileRow({
  profile,
  action,
  selected,
  mutating,
  onSelect,
  onRun,
  onArchive,
  onClone,
  onShare,
  onRevoke,
}: {
  profile: Profile;
  action?: ProfileAction;
  selected: boolean;
  mutating?: Problem | true;
  onSelect: () => void;
  onRun: (profile: Profile, kind: "launch" | "stop") => void;
  onArchive: (profile: Profile) => void;
  onClone: () => void;
  onShare: () => void;
  onRevoke: () => void;
}) {
  const state = projectedState(profile, action);
  const launchable = !profile.archived && canLaunch(state);
  const stoppable = !profile.archived && canStop(state);
  const archiveDisabled =
    Boolean(action) || mutating === true || state === "running";
  return (
    <tr
      className={selected ? "profile-row profile-row-selected" : "profile-row"}
      onClick={onSelect}
    >
      <td className="check-column">
        <input
          type="checkbox"
          aria-label={`Select ${profile.name}`}
          checked={selected}
          onChange={onSelect}
          onClick={(event) => event.stopPropagation()}
        />
      </td>
      <td>
        <div className="profile-cell">
          <div className="profile-mark">
            {profile.name.slice(0, 2).toUpperCase()}
          </div>
          <div>
            <strong>{profile.name}</strong>
            <div className="mono">{profile.id}</div>
            {mutating && mutating !== true ? (
              <div className="row-problem" role="alert">
                {mutating.detail ?? mutating.title}
              </div>
            ) : null}
          </div>
        </div>
      </td>
      <td>
        <span className="muted">No tags</span>
      </td>
      <td>
        <span className="muted">No note</span>
      </td>
      <td>
        <span className="muted">Not selected</span>
      </td>
      <td>
        <div>{profile.runtime}</div>
        <div className="muted">
          {profile.runtimeVersion ?? "Authority default"}
        </div>
      </td>
      <td>
        <Status
          label={profile.archived ? "Archived" : stateLabel(state)}
          tone={profile.archived ? "neutral" : stateTone(state)}
          live={Boolean(action) || mutating === true}
        />
      </td>
      <td className="row-actions">
        <Button
          variant={stoppable ? "secondary" : "primary"}
          disabled={
            profile.archived ||
            (!launchable && !stoppable) ||
            Boolean(action) ||
            mutating === true
          }
          loading={Boolean(action)}
          onClick={(event) => {
            event.stopPropagation();
            void onRun(profile, stoppable ? "stop" : "launch");
          }}
        >
          {stoppable ? "Stop" : "Launch"}
        </Button>
        <Button
          variant="quiet"
          disabled={archiveDisabled}
          loading={mutating === true}
          onClick={(event) => {
            event.stopPropagation();
            void onArchive(profile);
          }}
        >
          {profile.archived ? "Restore" : "Archive"}
        </Button>
        <Button
          variant="quiet"
          disabled={profile.archived || Boolean(action) || mutating === true}
          onClick={(event) => {
            event.stopPropagation();
            onClone();
          }}
        >
          Clone
        </Button>
        <Button
          variant="quiet"
          disabled={profile.archived || Boolean(action) || mutating === true}
          onClick={(event) => {
            event.stopPropagation();
            onShare();
          }}
        >
          Share
        </Button>
        <Button
          variant="danger"
          disabled={profile.archived || Boolean(action) || mutating === true}
          onClick={(event) => {
            event.stopPropagation();
            onRevoke();
          }}
        >
          Revoke
        </Button>
      </td>
    </tr>
  );
}

function Inspector({
  profile,
  action,
  onSyncRecovery,
}: {
  profile?: Profile;
  action?: ProfileAction;
  onSyncRecovery: () => void;
}) {
  if (!profile)
    return (
      <Panel
        className="inspector-panel"
        title="Selected profile"
        description="Select a profile to inspect runtime and sync state."
      />
    );
  return (
    <Panel
      className="inspector-panel"
      title="Selected profile"
      description={profile.name}
    >
      <div className="detail-grid">
        <Detail label="Profile ID" value={profile.id} />
        <Detail
          label="Runtime"
          value={`${profile.runtime} ${profile.runtimeVersion ?? "unresolved"}`}
        />
        <Detail label="Storage" value="Encrypted profile data" />
        <Detail
          label="Operation"
          value={action ? `${action.kind} in progress` : "Ready for action"}
        />
      </div>
      <div className="drawer-actions">
        <Button variant="quiet" onClick={onSyncRecovery}>
          Sync recovery
        </Button>
      </div>
    </Panel>
  );
}

function SyncRecoveryDrawer({
  api,
  profile,
  onClose,
}: {
  api: DesktopApi;
  profile: Profile;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<
    "overview" | "show" | "recover" | "recovered"
  >("overview");
  const [recoveryCode, setRecoveryCode] = useState("");
  const [enteredCode, setEnteredCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<Problem>();

  const reveal = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      setRecoveryCode(await api.getProfileSyncRecoveryCode(profile.id));
      setMode("show");
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setBusy(false);
    }
  };
  const recover = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setProblem(undefined);
    try {
      await api.recoverProfileSync(profile.id, enteredCode.trim());
      setEnteredCode("");
      setMode("recovered");
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Drawer title="Profile sync recovery" onClose={onClose}>
      <div className="drawer-form">
        <div className="drawer-intro">
          This recovery code opens only this profile's encrypted data. Keep it
          in a secure place outside this device.
        </div>
        {mode === "overview" ? (
          <>
            <div className="readonly-field">
              <span>Profile</span>
              <strong>{profile.name}</strong>
              <small>{profile.id}</small>
            </div>
            <div className="drawer-actions">
              <Button onClick={() => setMode("recover")}>
                Recover this device
              </Button>
              <Button
                variant="primary"
                loading={busy}
                onClick={() => void reveal()}
              >
                Show recovery code
              </Button>
            </div>
          </>
        ) : null}
        {mode === "show" ? (
          <>
            <Field
              label="Profile recovery code"
              value={recoveryCode}
              readOnly
              onFocus={(event) => event.currentTarget.select()}
              help="Select and copy this code to a password manager or another secure offline store."
            />
            <div className="inline-error" role="note">
              Anyone with this code can decrypt this profile's synced browser
              data.
            </div>
            <div className="drawer-actions">
              <Button
                onClick={() => {
                  setRecoveryCode("");
                  setMode("overview");
                }}
              >
                Done
              </Button>
            </div>
          </>
        ) : null}
        {mode === "recover" ? (
          <form
            className="drawer-form"
            style={{ padding: 0 }}
            onSubmit={recover}
          >
            <Field
              label="Recovery code"
              type="password"
              autoComplete="off"
              maxLength={64}
              minLength={64}
              required
              value={enteredCode}
              onChange={(event) => setEnteredCode(event.currentTarget.value)}
              help="Enter the 64-character code saved from another trusted device."
            />
            <div className="drawer-actions">
              <Button
                type="button"
                onClick={() => {
                  setEnteredCode("");
                  setProblem(undefined);
                  setMode("overview");
                }}
              >
                Back
              </Button>
              <Button
                type="submit"
                variant="primary"
                loading={busy}
                disabled={enteredCode.trim().length !== 64}
              >
                Recover keys
              </Button>
            </div>
          </form>
        ) : null}
        {mode === "recovered" ? (
          <>
            <div className="readonly-field">
              <strong>Workspace keys recovered on this device.</strong>
              <small>
                Retry launching the profile to download and decrypt its latest
                data.
              </small>
            </div>
            <div className="drawer-actions">
              <Button variant="primary" onClick={onClose}>
                Done
              </Button>
            </div>
          </>
        ) : null}
        {problem ? (
          <div className="inline-error" role="alert">
            {problem.detail ?? problem.title} · {problem.correlationId}
          </div>
        ) : null}
      </div>
    </Drawer>
  );
}
function Detail({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="detail-label">{label}</div>
      <div>{value}</div>
    </div>
  );
}
function Jobs({ jobs }: { jobs: Job[] }) {
  return (
    <Panel
      className="jobs-panel"
      title="Recent jobs"
      description="Durable workspace operations"
    >
      <div className="job-list">
        {jobs.length === 0 ? (
          <div className="muted">No operations recorded in this session.</div>
        ) : (
          jobs.slice(0, 3).map((job) => (
            <div className={`job-row job-${job.status}`} key={job.id}>
              <div>
                <strong>
                  {job.kind[0].toUpperCase() + job.kind.slice(1)} ·{" "}
                  {job.profile}
                </strong>
                <div className="muted">{job.detail}</div>
              </div>
              <Status
                label={job.status}
                tone={
                  job.status === "succeeded"
                    ? "success"
                    : job.status === "failed"
                      ? "danger"
                      : "warning"
                }
              />
            </div>
          ))
        )}
      </div>
    </Panel>
  );
}
function ConnectionStatus({ state }: { state: LoadState }) {
  const offline = state.kind === "offline" || state.kind === "error";
  return (
    <span className={offline ? "connection connection-warning" : "connection"}>
      <span className="status-dot" />
      {offline ? "Offline" : "Connected"}
    </span>
  );
}
function StateNotice({
  state,
  onRetry,
}: {
  state: LoadState;
  onRetry: () => void;
}) {
  if (state.kind !== "offline" && state.kind !== "error") return null;
  return (
    <div className="state-notice state-notice-error" role="alert">
      <div>
        <strong>
          {state.kind === "offline"
            ? "Offline mode."
            : "Profiles could not refresh."}
        </strong>
        <span>
          {state.kind === "offline"
            ? "Existing local data remains available."
            : (state.problem.detail ?? state.problem.title)}
        </span>
      </div>
      <Button onClick={onRetry}>Retry</Button>
    </div>
  );
}
function EmptyState({
  state,
  onCreate,
}: {
  state: LoadState;
  onCreate: () => void;
}) {
  if (state.kind === "loading")
    return <div className="empty-state">Loading profiles…</div>;
  return (
    <div className="empty-state">
      <strong>No profiles match this view.</strong>
      <span>Create the first profile or change the filters.</span>
      <Button onClick={onCreate}>＋ New profile</Button>
    </div>
  );
}

function CreateDrawer({
  api,
  onClose,
  onCreated,
}: {
  api: DesktopApi;
  onClose: () => void;
  onCreated: () => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [runtimeVersion, setRuntimeVersion] = useState("");
  const [workspaceId, setWorkspaceId] = useState("");
  const [proxyId, setProxyId] = useState("");
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [proxies, setProxies] = useState<ProxyRecord[]>([]);
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<Problem>();

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const organizations = await api.listOrganizations();
        const pages = await Promise.all(
          organizations.items.map((organization) =>
            api.listWorkspaces(organization.id),
          ),
        );
        const items = pages.flatMap((page) => page.items);
        if (active) {
          setWorkspaces(items);
          if (items.length === 1) setWorkspaceId(items[0].id);
        }
      } catch (error) {
        if (active) setProblem(normalizeProblem(error));
      }
    })();
    return () => {
      active = false;
    };
  }, [api]);

  useEffect(() => {
    let active = true;
    setProxyId("");
    if (!workspaceId) {
      setProxies([]);
      return () => {
        active = false;
      };
    }
    void api
      .listProxies(workspaceId)
      .then((page) => {
        if (active) setProxies(page.items);
      })
      .catch((error) => {
        if (active) setProblem(normalizeProblem(error));
      });
    return () => {
      active = false;
    };
  }, [api, workspaceId]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!name.trim()) {
      setProblem(fieldProblem("Profile name is required."));
      return;
    }
    if (workspaces.length > 1 && !workspaceId) {
      setProblem(fieldProblem("Select a workspace."));
      return;
    }
    setSaving(true);
    setProblem(undefined);
    try {
      const input: ProfileCreateInput = {
        name: name.trim(),
        runtime: "wayfern",
        ...(runtimeVersion.trim()
          ? { runtimeVersion: runtimeVersion.trim() }
          : {}),
        ...(workspaceId ? { workspaceId } : {}),
        ...(proxyId ? { proxyId } : {}),
      };
      await api.createProfile(input);
      await onCreated();
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Drawer title="New profile" onClose={onClose}>
      <form className="drawer-form" onSubmit={submit}>
        <div className="drawer-intro">
          Create an isolated browser profile. Runtime and security capabilities
          remain explicit.
        </div>
        <Field
          label="Profile name"
          required
          autoFocus
          value={name}
          onChange={(event) => setName(event.currentTarget.value)}
          error={problem?.code === "invalid_input" ? problem.detail : undefined}
        />
        {workspaces.length > 0 ? (
          <label className="select-field">
            <span>Workspace</span>
            <select
              value={workspaceId}
              onChange={(event) => setWorkspaceId(event.currentTarget.value)}
              required={workspaces.length > 1}
            >
              <option value="">Select workspace</option>
              {workspaces.map((workspace) => (
                <option key={workspace.id} value={workspace.id}>
                  {workspace.name}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        {workspaceId ? (
          <label className="select-field">
            <span>Proxy</span>
            <select
              value={proxyId}
              onChange={(event) => setProxyId(event.currentTarget.value)}
            >
              <option value="">No proxy</option>
              {proxies.map((record) => (
                <option key={record.proxy.id} value={record.proxy.id}>
                  {record.proxy.name || record.proxy.id} ·{" "}
                  {record.proxy.protocol}://{record.proxy.host}:
                  {record.proxy.port}
                </option>
              ))}
            </select>
            <small className="field-help">
              Credentials remain sealed. Proxy assignment is workspace-scoped.
            </small>
          </label>
        ) : null}
        <Field
          label="Runtime version"
          value={runtimeVersion}
          onChange={(event) => setRuntimeVersion(event.currentTarget.value)}
          help="Leave blank for the authority-selected version."
        />
        <div className="readonly-field">
          <span>Runtime</span>
          <strong>Wayfern</strong>
          <small>Only verified runtime binaries can launch.</small>
        </div>
        {problem && problem.code !== "invalid_input" ? (
          <div className="inline-error" role="alert">
            {problem.detail ?? problem.title}
          </div>
        ) : null}
        <div className="drawer-actions">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="submit"
            variant="primary"
            loading={saving}
            loadingLabel="Creating…"
          >
            Create profile
          </Button>
        </div>
      </form>
    </Drawer>
  );
}

function CloneDrawer({
  api,
  profile,
  onClose,
  onCompleted,
}: {
  api: DesktopApi;
  profile: Profile;
  onClose: () => void;
  onCompleted: () => Promise<void>;
}) {
  const [name, setName] = useState(`${profile.name} (Copy)`);
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<Problem>();
  const [receipt, setReceipt] =
    useState<Awaited<ReturnType<DesktopApi["cloneProfile"]>>>();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const normalizedName = normalizedRequiredText(name);
    if (!normalizedName) {
      setProblem(fieldProblem("Enter a profile name."));
      return;
    }
    if (!usableRevision(profile.revision)) {
      setProblem(
        fieldProblem(
          "Profile revision is unavailable. Refresh before cloning.",
        ),
      );
      return;
    }
    setSaving(true);
    setProblem(undefined);
    try {
      const next = await api.cloneProfile({
        sourceProfileId: profile.id,
        targetProfileId: `profile_${crypto.randomUUID()}`,
        name: normalizedName,
        expectedRevision: profile.revision,
      });
      setReceipt(next);
      await onCompleted();
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Drawer title="Clone profile" onClose={onClose}>
      <form className="drawer-form" onSubmit={submit}>
        <div className="drawer-intro">
          Metadata-only clone. Cookies and other secrets remain excluded by
          authority.
        </div>
        <div className="readonly-field">
          <span>Source profile</span>
          <strong>{profile.name}</strong>
          <small className="mono">{profile.id}</small>
        </div>
        <Field
          label="New profile name"
          required
          autoFocus
          value={name}
          onChange={(event) => setName(event.currentTarget.value)}
          error={problem?.code === "invalid_input" ? problem.detail : undefined}
        />
        {!usableRevision(profile.revision) ? (
          <div className="inline-error" role="alert">
            Profile revision unavailable. Close and refresh before retrying.
          </div>
        ) : null}
        {problem && problem.code !== "invalid_input" ? (
          <div className="inline-error" role="alert">
            {problem.detail ?? problem.title}
          </div>
        ) : null}
        {receipt ? (
          <div className="workflow-success" role="status">
            <strong>Clone confirmed.</strong>
            <span>
              {receipt.profile.name} · {receipt.profile.id}
            </span>
          </div>
        ) : null}
        <div className="drawer-actions">
          {receipt ? (
            <Button type="button" variant="primary" onClick={onClose}>
              Done
            </Button>
          ) : (
            <>
              <Button type="button" onClick={onClose}>
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                loading={saving}
                loadingLabel="Cloning…"
              >
                Clone profile
              </Button>
            </>
          )}
        </div>
      </form>
    </Drawer>
  );
}

function ShareDrawer({
  api,
  profile,
  onClose,
  onCompleted,
}: {
  api: DesktopApi;
  profile: Profile;
  onClose: () => void;
  onCompleted: () => Promise<void>;
}) {
  const [memberId, setMemberId] = useState("");
  const [permission, setPermission] = useState<"read" | "operate">("read");
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<Problem>();
  const [receipt, setReceipt] =
    useState<Awaited<ReturnType<DesktopApi["shareProfile"]>>>();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const normalizedMemberId = normalizedRequiredText(memberId);
    if (!normalizedMemberId) {
      setProblem(fieldProblem("Enter a member ID."));
      return;
    }
    if (!usableRevision(profile.revision)) {
      setProblem(
        fieldProblem(
          "Profile revision is unavailable. Refresh before sharing.",
        ),
      );
      return;
    }
    setSaving(true);
    setProblem(undefined);
    try {
      const next = await api.shareProfile({
        profileId: profile.id,
        memberId: normalizedMemberId,
        permission,
        expectedRevision: profile.revision,
      });
      setReceipt(next);
      await onCompleted();
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Drawer title="Share profile" onClose={onClose}>
      <form className="drawer-form" onSubmit={submit}>
        <div className="drawer-intro">
          Enter an existing member ID. No member directory or guessed identity
          is used here.
        </div>
        <div className="readonly-field">
          <span>Profile</span>
          <strong>{profile.name}</strong>
          <small className="mono">{profile.id}</small>
        </div>
        <Field
          label="Member ID"
          required
          autoFocus
          value={memberId}
          onChange={(event) => setMemberId(event.currentTarget.value)}
          error={problem?.code === "invalid_input" ? problem.detail : undefined}
        />
        <label className="select-field">
          <span>Permission</span>
          <select
            value={permission}
            onChange={(event) =>
              setPermission(event.currentTarget.value as "read" | "operate")
            }
          >
            <option value="read">Read</option>
            <option value="operate">Operate</option>
          </select>
        </label>
        {!usableRevision(profile.revision) ? (
          <div className="inline-error" role="alert">
            Profile revision unavailable. Close and refresh before retrying.
          </div>
        ) : null}
        {problem && problem.code !== "invalid_input" ? (
          <div className="inline-error" role="alert">
            {problem.detail ?? problem.title}
          </div>
        ) : null}
        {receipt ? (
          <div className="workflow-success" role="status">
            <strong>
              {receipt.shared ? "Share confirmed." : "Share not confirmed."}
            </strong>
            <span>
              {receipt.memberId} · {receipt.permission ?? permission}
            </span>
          </div>
        ) : null}
        <div className="drawer-actions">
          {receipt ? (
            <Button type="button" variant="primary" onClick={onClose}>
              Done
            </Button>
          ) : (
            <>
              <Button type="button" onClick={onClose}>
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                loading={saving}
                loadingLabel="Sharing…"
              >
                Share profile
              </Button>
            </>
          )}
        </div>
      </form>
    </Drawer>
  );
}

function RevokeDrawer({
  api,
  profile,
  onClose,
  onCompleted,
}: {
  api: DesktopApi;
  profile: Profile;
  onClose: () => void;
  onCompleted: () => Promise<void>;
}) {
  const [memberId, setMemberId] = useState("");
  const [permission, setPermission] = useState<"read" | "operate">("read");
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<Problem>();
  const [receipt, setReceipt] =
    useState<Awaited<ReturnType<DesktopApi["revokeProfileShare"]>>>();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const normalizedMemberId = normalizedRequiredText(memberId);
    if (!normalizedMemberId) {
      setProblem(fieldProblem("Enter a member ID."));
      return;
    }
    if (!usableRevision(profile.revision)) {
      setProblem(
        fieldProblem(
          "Profile revision is unavailable. Refresh before revoking access.",
        ),
      );
      return;
    }
    setSaving(true);
    setProblem(undefined);
    try {
      const next = await api.revokeProfileShare({
        profileId: profile.id,
        memberId: normalizedMemberId,
        permission,
        expectedRevision: profile.revision,
      });
      setReceipt(next);
      await onCompleted();
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Drawer title="Revoke profile access" onClose={onClose}>
      <form className="drawer-form" onSubmit={submit}>
        <div className="drawer-intro">
          Revocation is authoritative and may remove access immediately. Confirm
          the exact member ID and permission.
        </div>
        <div className="readonly-field">
          <span>Profile</span>
          <strong>{profile.name}</strong>
          <small className="mono">{profile.id}</small>
        </div>
        <Field
          label="Member ID"
          required
          autoFocus
          value={memberId}
          onChange={(event) => setMemberId(event.currentTarget.value)}
          error={problem?.code === "invalid_input" ? problem.detail : undefined}
        />
        <label className="select-field">
          <span>Permission to revoke</span>
          <select
            value={permission}
            onChange={(event) =>
              setPermission(event.currentTarget.value as "read" | "operate")
            }
          >
            <option value="read">Read</option>
            <option value="operate">Operate</option>
          </select>
        </label>
        {!usableRevision(profile.revision) ? (
          <div className="inline-error" role="alert">
            Profile revision unavailable. Close and refresh before retrying.
          </div>
        ) : null}
        {problem && problem.code !== "invalid_input" ? (
          <div className="inline-error" role="alert">
            {problem.detail ?? problem.title}
          </div>
        ) : null}
        {receipt ? (
          <div className="workflow-success" role="status">
            <strong>
              {receipt.revoked
                ? "Revocation confirmed."
                : "Revocation not confirmed."}
            </strong>
            <span>
              {receipt.memberId} · {permission}
            </span>
          </div>
        ) : null}
        <div className="drawer-actions">
          {receipt ? (
            <Button type="button" variant="primary" onClick={onClose}>
              Done
            </Button>
          ) : (
            <>
              <Button type="button" onClick={onClose}>
                Cancel
              </Button>
              <Button
                type="submit"
                variant="danger"
                loading={saving}
                loadingLabel="Revoking…"
              >
                Revoke access
              </Button>
            </>
          )}
        </div>
      </form>
    </Drawer>
  );
}

function ProxyDrawer({
  api,
  onClose,
}: {
  api: DesktopApi;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<"inventory" | "parse">("inventory");
  const [text, setText] = useState("");
  const [report, setReport] = useState<ProxyParseReport>();
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [workspaceId, setWorkspaceId] = useState("");
  const [records, setRecords] = useState<ProxyRecord[]>([]);
  const [editing, setEditing] = useState<ProxyRecord>();
  const [editorOpen, setEditorOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [problem, setProblem] = useState<string>();

  const loadInventory = useCallback(
    async (selectedWorkspaceId: string) => {
      if (!selectedWorkspaceId) return;
      setLoading(true);
      setProblem(undefined);
      try {
        setRecords((await api.listProxies(selectedWorkspaceId)).items);
      } catch (error) {
        setProblem(
          normalizeProblem(error).detail ?? "Proxy inventory failed to load.",
        );
      } finally {
        setLoading(false);
      }
    },
    [api],
  );

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const organizations = await api.listOrganizations();
        const pages = await Promise.all(
          organizations.items.map((organization) =>
            api.listWorkspaces(organization.id),
          ),
        );
        const items = pages.flatMap((page) => page.items);
        if (!active) return;
        setWorkspaces(items);
        const firstWorkspaceId = items[0]?.id ?? "";
        setWorkspaceId(firstWorkspaceId);
        if (firstWorkspaceId) await loadInventory(firstWorkspaceId);
      } catch (error) {
        if (active)
          setProblem(
            normalizeProblem(error).detail ?? "Workspaces failed to load.",
          );
      }
    })();
    return () => {
      active = false;
    };
  }, [api, loadInventory]);

  const parse = async () => {
    if (!text.trim()) {
      setProblem("Paste at least one proxy row.");
      return;
    }
    setLoading(true);
    try {
      setProblem(undefined);
      setReport(await api.parseProxy(text));
    } catch (error) {
      setProblem(normalizeProblem(error).detail ?? "Proxy parsing failed.");
    } finally {
      setLoading(false);
    }
  };

  const saveProxy = async (input: ProxyCreateInput | ProxyUpdateInput) => {
    setLoading(true);
    setProblem(undefined);
    try {
      if ("expectedRevision" in input) await api.updateProxy(input);
      else await api.createProxy(input);
      setEditorOpen(false);
      setEditing(undefined);
      await loadInventory(workspaceId);
    } catch (error) {
      setProblem(normalizeProblem(error).detail ?? "Proxy save failed.");
    } finally {
      setLoading(false);
    }
  };

  const removeProxy = async (record: ProxyRecord) => {
    if (
      !window.confirm(`Delete proxy ${record.proxy.name || record.proxy.id}?`)
    )
      return;
    setLoading(true);
    setProblem(undefined);
    try {
      await api.deleteProxy({
        workspaceId: record.workspaceId,
        proxyId: record.proxy.id,
        expectedRevision: record.revision,
      });
      await loadInventory(workspaceId);
    } catch (error) {
      setProblem(normalizeProblem(error).detail ?? "Proxy deletion failed.");
    } finally {
      setLoading(false);
    }
  };

  return (
    <Drawer title="Proxy library" onClose={onClose}>
      <div className="drawer-form">
        {editorOpen ? (
          <ProxyEditor
            editing={editing}
            workspaceId={workspaceId}
            loading={loading}
            onCancel={() => {
              setEditorOpen(false);
              setEditing(undefined);
            }}
            onSubmit={(input) => void saveProxy(input)}
          />
        ) : null}
        <div
          className="drawer-tabs"
          role="tablist"
          aria-label="Proxy library view"
        >
          <button
            className={
              mode === "inventory"
                ? "drawer-tab drawer-tab-active"
                : "drawer-tab"
            }
            type="button"
            role="tab"
            aria-selected={mode === "inventory"}
            onClick={() => setMode("inventory")}
          >
            Inventory
          </button>
          <button
            className={
              mode === "parse" ? "drawer-tab drawer-tab-active" : "drawer-tab"
            }
            type="button"
            role="tab"
            aria-selected={mode === "parse"}
            onClick={() => setMode("parse")}
          >
            Quick parse
          </button>
        </div>
        {mode === "inventory" ? (
          <>
            <label className="select-field">
              <span>Workspace</span>
              <select
                value={workspaceId}
                onChange={(event) => {
                  const next = event.currentTarget.value;
                  setWorkspaceId(next);
                  void loadInventory(next);
                }}
                disabled={loading || workspaces.length === 0}
              >
                <option value="">Select workspace</option>
                {workspaces.map((workspace) => (
                  <option key={workspace.id} value={workspace.id}>
                    {workspace.name}
                  </option>
                ))}
              </select>
            </label>
            <div className="proxy-inventory-header">
              <div>
                <strong>{records.length} proxies</strong>
                <span>Hosted metadata. Credentials remain sealed.</span>
              </div>
              <div className="drawer-actions">
                <Button
                  disabled={!workspaceId || loading}
                  onClick={() => {
                    setEditing(undefined);
                    setEditorOpen(true);
                  }}
                >
                  Add proxy
                </Button>
                <Button
                  disabled={!workspaceId || loading}
                  loading={loading}
                  onClick={() => void loadInventory(workspaceId)}
                >
                  Refresh
                </Button>
              </div>
            </div>
            {problem ? (
              <div className="inline-error" role="alert">
                {problem}
              </div>
            ) : null}
            {!loading && !problem && records.length === 0 ? (
              <div className="proxy-empty">
                <strong>No proxies in this workspace.</strong>
                <span>
                  Use Quick parse to validate rows before import support is
                  enabled.
                </span>
              </div>
            ) : null}
            <div className="proxy-list">
              {records.map((record) => (
                <div className="proxy-record" key={record.proxy.id}>
                  <div className="proxy-record-main">
                    <strong>{record.proxy.name || record.proxy.id}</strong>
                    <span className="mono">
                      {record.proxy.protocol}://{record.proxy.host}:
                      {record.proxy.port}
                    </span>
                  </div>
                  <div className="proxy-record-meta">
                    <Status
                      label={
                        record.proxy.hasCredential
                          ? "Credential sealed"
                          : "No credential"
                      }
                      tone={record.proxy.hasCredential ? "success" : "neutral"}
                    />
                    <span>r{record.revision}</span>
                    <span className="proxy-record-actions">
                      <Button
                        variant="quiet"
                        disabled={loading}
                        onClick={() => {
                          setEditing(record);
                          setEditorOpen(true);
                        }}
                      >
                        Edit
                      </Button>
                      <Button
                        variant="danger"
                        disabled={loading || record.proxy.rotationConfigured}
                        onClick={() => void removeProxy(record)}
                      >
                        Delete
                      </Button>
                    </span>
                  </div>
                </div>
              ))}
            </div>
          </>
        ) : (
          <>
            <div className="drawer-intro">
              Validate and normalize proxy rows without exposing credentials.
            </div>
            <Field
              label="Proxy rows"
              multiline
              value={text}
              onChange={(event) => setText(event.currentTarget.value)}
              help="One proxy per line. Credentials stay redacted in results."
            />
            <div className="drawer-actions">
              <Button
                variant="primary"
                loading={loading}
                onClick={() => void parse()}
              >
                Parse input
              </Button>
            </div>
            {problem ? (
              <div className="inline-error" role="alert">
                {problem}
              </div>
            ) : null}
            {report ? (
              <div className="parse-summary">
                <strong>{report.validCount ?? 0} valid</strong>
                <span>
                  {report.invalidCount ?? 0} invalid ·{" "}
                  {report.duplicateCount ?? 0} duplicate
                </span>
                {report.rows?.map((row) => (
                  <div className="parse-row" key={row.line}>
                    <span>Line {row.line}</span>
                    <span
                      className={row.valid ? "parse-valid" : "parse-invalid"}
                    >
                      {row.valid
                        ? (row.endpoint ?? "Valid")
                        : (row.error ?? "Invalid")}
                    </span>
                  </div>
                ))}
              </div>
            ) : null}
          </>
        )}
      </div>
    </Drawer>
  );
}

function ProxyEditor({
  editing,
  workspaceId,
  loading,
  onCancel,
  onSubmit,
}: {
  editing?: ProxyRecord;
  workspaceId: string;
  loading: boolean;
  onCancel: () => void;
  onSubmit: (input: ProxyCreateInput | ProxyUpdateInput) => void;
}) {
  const [id, setId] = useState(editing?.proxy.id ?? "");
  const [name, setName] = useState(editing?.proxy.name ?? "");
  const [protocol, setProtocol] = useState<ProxyProtocol>(
    editing?.proxy.protocol ?? "http",
  );
  const [host, setHost] = useState(editing?.proxy.host ?? "");
  const [port, setPort] = useState(String(editing?.proxy.port ?? ""));
  const [bypassRules, setBypassRules] = useState(
    editing?.proxy.bypassRules.join("\n") ?? "",
  );
  const [validation, setValidation] = useState<string>();

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const parsedPort = Number(port);
    if (
      !id.trim() ||
      !host.trim() ||
      !Number.isInteger(parsedPort) ||
      parsedPort < 1 ||
      parsedPort > 65535
    ) {
      setValidation("Enter an ID, host, and port from 1 to 65535.");
      return;
    }
    const form = event.currentTarget;
    const formData = new FormData(form);
    const username = String(formData.get("proxyUsername") ?? "");
    const password = String(formData.get("proxyPassword") ?? "");
    const clearCredential = formData.get("clearCredential") === "on";
    if (username.length > 0 !== password.length > 0) {
      setValidation("Enter both username and password, or leave both empty.");
      return;
    }
    const credential =
      username && password ? { username, password } : undefined;
    if (clearCredential && credential) {
      setValidation(
        "Clear the saved credential or enter a replacement, not both.",
      );
      return;
    }
    const now = new Date().toISOString();
    const proxy: ProxyMetadataInput = {
      id: id.trim(),
      ...(name.trim() ? { name: name.trim() } : {}),
      protocol,
      host: host.trim(),
      port: parsedPort,
      bypassRules: bypassRules
        .split("\n")
        .map((rule) => rule.trim())
        .filter(Boolean),
      createdAt: editing?.proxy.createdAt ?? now,
      updatedAt: now,
    };
    const submission = editing
      ? {
          workspaceId,
          proxyId: editing.proxy.id,
          expectedRevision: editing.revision,
          proxy,
          ...(credential ? { credential } : {}),
          ...(clearCredential ? { clearCredential: true } : {}),
        }
      : { workspaceId, proxy, ...(credential ? { credential } : {}) };
    const usernameInput = form.elements.namedItem("proxyUsername");
    const passwordInput = form.elements.namedItem("proxyPassword");
    if (usernameInput instanceof HTMLInputElement) usernameInput.value = "";
    if (passwordInput instanceof HTMLInputElement) passwordInput.value = "";
    onSubmit(submission);
  };

  return (
    <div className="proxy-editor">
      <div className="drawer-intro">
        {editing ? "Edit proxy metadata." : "Add proxy metadata."} Credentials
        are stored in this device’s operating system vault and encrypted before
        sync.
      </div>
      <form className="drawer-form" onSubmit={submit}>
        <Field
          label="ID"
          value={id}
          disabled={Boolean(editing)}
          onChange={(event) => setId(event.currentTarget.value)}
        />
        <Field
          label="Name"
          value={name}
          onChange={(event) => setName(event.currentTarget.value)}
        />
        <label className="select-field">
          <span>Protocol</span>
          <select
            value={protocol}
            onChange={(event) =>
              setProtocol(event.currentTarget.value as ProxyProtocol)
            }
          >
            <option value="http">http</option>
            <option value="https">https</option>
            <option value="socks4">socks4</option>
            <option value="socks5">socks5</option>
          </select>
        </label>
        <Field
          label="Host"
          value={host}
          onChange={(event) => setHost(event.currentTarget.value)}
        />
        <Field
          label="Port"
          value={port}
          onChange={(event) => setPort(event.currentTarget.value)}
        />
        <Field
          label="Proxy username"
          name="proxyUsername"
          autoComplete="username"
          help={
            editing?.proxy.hasCredential
              ? "Leave both credential fields empty to keep the saved credential."
              : undefined
          }
        />
        <Field
          label="Proxy password"
          name="proxyPassword"
          type="password"
          autoComplete="new-password"
        />
        {editing?.proxy.hasCredential ? (
          <label className="select-field">
            <span>
              <input type="checkbox" name="clearCredential" /> Remove saved
              credential
            </span>
          </label>
        ) : null}
        <Field
          label="Bypass rules"
          multiline
          value={bypassRules}
          onChange={(event) => setBypassRules(event.currentTarget.value)}
          help="One rule per line."
          error={validation}
        />
        <div className="drawer-actions">
          <Button type="button" onClick={onCancel} disabled={loading}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" loading={loading}>
            {editing ? "Save proxy" : "Create proxy"}
          </Button>
        </div>
      </form>
    </div>
  );
}

function Drawer({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: React.ReactNode;
}) {
  const titleId = useId();
  const drawerRef = useRef<HTMLElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);

  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    previousFocusRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const drawer = drawerRef.current;
    const focusable = drawer?.querySelector<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
    );
    focusable?.focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== "Tab" || !drawer) return;
      const elements = [
        ...drawer.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
        ),
      ];
      if (elements.length === 0) {
        event.preventDefault();
        return;
      }
      const first = elements[0];
      const last = elements[elements.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      if (previousFocusRef.current?.isConnected)
        previousFocusRef.current.focus();
    };
  }, []);

  return (
    <div className="drawer-backdrop" role="presentation">
      <aside
        ref={drawerRef}
        className="drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <header className="drawer-header">
          <div>
            <div className="eyebrow">Operation</div>
            <h2 id={titleId}>{title}</h2>
          </div>
          <Button variant="quiet" onClick={onClose} aria-label="Close">
            Close
          </Button>
        </header>
        {children}
      </aside>
    </div>
  );
}
async function waitForOperation(api: DesktopApi, operationId: string) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const operation = await api.getOperation(operationId);
    if (operation.status === "completed") return;
    if (operation.status === "failed" || operation.status === "cancelled")
      throw operation.error ?? new Error(`Operation ${operation.status}`);
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error("Operation status timed out");
}
function fieldProblem(detail: string): Problem {
  return {
    type: "about:blank",
    title: "Check the form",
    status: 400,
    code: "invalid_input",
    correlationId: "client_validation",
    detail,
  };
}
function stateLabel(state: ProfileState) {
  return state[0].toUpperCase() + state.slice(1);
}
function stateTone(state: ProfileState) {
  if (state === "running" || state === "ready") return "success" as const;
  if (state === "failed") return "danger" as const;
  if (state === "launching" || state === "stopping" || state === "recovering")
    return "warning" as const;
  return "neutral" as const;
}
