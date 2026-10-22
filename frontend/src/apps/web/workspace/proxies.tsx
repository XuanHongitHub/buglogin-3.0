import { useMemo, useState } from "react";
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
  errorMessage,
  type MutationOptions,
  type ProxyCreateInput,
  type ProxyMetadataInput,
  type ProxyPage,
  type ProxyProtocol,
  type ProxyRecord,
  type ProxyUpdateInput,
  type WebApiClient,
} from "../api";
import { type Column, EnterpriseDataTable } from "./data-table";

const protocols: ProxyProtocol[] = ["http", "https", "socks4", "socks5"];

export function ProxyInventory({
  api,
  page,
  workspaceId,
  retry,
}: {
  api: WebApiClient;
  page: ProxyPage;
  workspaceId: string;
  retry: () => void;
}) {
  const [editing, setEditing] = useState<ProxyRecord>();
  const [formOpen, setFormOpen] = useState(false);
  const [mutation, setMutation] = useState<"idle" | "loading" | "error">(
    "idle",
  );
  const [message, setMessage] = useState<string>();
  const [error, setError] = useState<string>();
  const rows = useMemo(() => page.items, [page.items]);
  const submit = async (input: ProxyCreateInput | ProxyUpdateInput) => {
    setMutation("loading");
    setMessage(undefined);
    setError(undefined);
    try {
      const options: MutationOptions = {
        idempotencyKey: `${editing ? "proxy-update" : "proxy-create"}-${input.proxy.id}`,
        signal: new AbortController().signal,
      };
      if ("expectedRevision" in input)
        await api.writes.updateProxy(input, options);
      else {
        const result = await api.writes.createProxy(input, options);
        setMessage(
          result.deduplicated
            ? "Existing proxy returned; no duplicate was created."
            : "Proxy created.",
        );
      }
      setMutation("idle");
      setEditing(undefined);
      setFormOpen(false);
      retry();
    } catch (cause) {
      setMutation("error");
      setError(errorMessage(cause));
    }
  };
  const remove = async (record: ProxyRecord) => {
    setMutation("loading");
    setMessage(undefined);
    setError(undefined);
    try {
      await api.writes.deleteProxy(
        {
          workspaceId,
          proxyId: record.proxy.id,
          expectedRevision: record.revision,
        },
        {
          idempotencyKey: `proxy-delete-${record.proxy.id}`,
          signal: new AbortController().signal,
        },
      );
      setMutation("idle");
      setMessage("Proxy deleted.");
      retry();
    } catch (cause) {
      setMutation("error");
      setError(errorMessage(cause));
    }
  };
  const columns: Column<ProxyRecord>[] = [
    {
      key: "proxy",
      header: "Proxy",
      sortable: true,
      sortValue: (record) => record.proxy.name || record.proxy.id,
      render: (record) => (
        <div>
          <strong style={{ ...typography.bodySmall, display: "block" }}>
            {record.proxy.name || record.proxy.id}
          </strong>
          <code style={{ fontSize: "11px", color: colors.text.muted }}>
            {record.proxy.id}
          </code>
        </div>
      ),
    },
    {
      key: "endpoint",
      header: "Endpoint",
      sortable: true,
      sortValue: (record) => `${record.proxy.host}:${record.proxy.port}`,
      render: (record) => (
        <span style={{ fontFamily: typography.mono, fontSize: 13 }}>
          {record.proxy.protocol}://{record.proxy.host}:{record.proxy.port}
        </span>
      ),
    },
    {
      key: "credentials",
      header: "Credentials",
      sortable: true,
      sortValue: (record) => (record.proxy.hasCredential ? 1 : 0),
      render: (record) => (
        <Status
          label={record.proxy.hasCredential ? "Sealed" : "None"}
          tone={record.proxy.hasCredential ? "success" : "neutral"}
        />
      ),
    },
    {
      key: "assignment",
      header: "Assignment",
      sortable: true,
      sortValue: (record) => (record.proxy.rotationConfigured ? 1 : 0),
      render: (record) => (
        <Status
          label={
            record.proxy.rotationConfigured
              ? "Rotation configured"
              : "Available"
          }
          tone={record.proxy.rotationConfigured ? "info" : "neutral"}
        />
      ),
    },
    {
      key: "revision",
      header: "Revision",
      sortable: true,
      sortValue: (record) => record.revision,
      render: (record) => (
        <span style={{ color: colors.text.muted }}>r{record.revision}</span>
      ),
    },
    {
      key: "actions",
      header: "Actions",
      render: (record) => (
        <span style={{ display: "inline-flex", gap: spacing[2] }}>
          <Button
            onClick={() => {
              setEditing(record);
              setFormOpen(true);
            }}
            disabled={mutation === "loading"}
          >
            Edit
          </Button>
          <Button
            variant="danger"
            onClick={() => void remove(record)}
            disabled={mutation === "loading" || record.proxy.rotationConfigured}
          >
            Delete
          </Button>
        </span>
      ),
    },
  ];
  return (
    <div style={{ display: "grid", gap: spacing[4] }}>
      <Panel
        title="Proxy library"
        description="Workspace-scoped proxy metadata. Credentials remain sealed and server-authorized."
        actions={
          <div style={{ display: "flex", gap: spacing[2] }}>
            <Button
              variant="primary"
              onClick={() => {
                setEditing(undefined);
                setFormOpen(true);
              }}
            >
              Add proxy
            </Button>
            <Button onClick={retry}>Refresh</Button>
          </div>
        }
      >
        <EnterpriseDataTable<ProxyRecord>
          data={rows}
          columns={columns}
          getRowKey={(record) => record.proxy.id}
          searchPlaceholder="Search proxies by name, host, port, protocol or ID..."
          searchFilter={(record, needle) =>
            record.proxy.name?.toLowerCase().includes(needle) ||
            record.proxy.id.toLowerCase().includes(needle) ||
            record.proxy.host.toLowerCase().includes(needle) ||
            String(record.proxy.port).includes(needle) ||
            record.proxy.protocol.toLowerCase().includes(needle)
          }
          filters={[
            {
              key: "protocol",
              label: "Protocol",
              options: [
                { label: "All protocols", value: "all" },
                { label: "HTTP", value: "http" },
                { label: "HTTPS", value: "https" },
                { label: "SOCKS4", value: "socks4" },
                { label: "SOCKS5", value: "socks5" },
              ],
            },
            {
              key: "credentials",
              label: "Credentials",
              options: [
                { label: "All credentials", value: "all" },
                { label: "Sealed credentials", value: "sealed" },
                { label: "Direct / None", value: "none" },
              ],
            },
          ]}
          filterPredicate={(record, filters) => {
            if (
              filters.protocol &&
              filters.protocol !== "all" &&
              record.proxy.protocol !== filters.protocol
            ) {
              return false;
            }
            if (filters.credentials && filters.credentials !== "all") {
              if (
                filters.credentials === "sealed" &&
                !record.proxy.hasCredential
              )
                return false;
              if (filters.credentials === "none" && record.proxy.hasCredential)
                return false;
            }
            return true;
          }}
          bulkActions={[
            {
              label: "Export selected (.csv)",
              variant: "secondary",
              action: (selected) => {
                const csv =
                  "proxy_id,name,protocol,host,port\n" +
                  selected
                    .map(
                      (r) =>
                        `"${r.proxy.id}","${r.proxy.name || ""}","${r.proxy.protocol}","${r.proxy.host}",${r.proxy.port}`,
                    )
                    .join("\n");
                const blob = new Blob([csv], { type: "text/csv" });
                const url = URL.createObjectURL(blob);
                const a = document.createElement("a");
                a.href = url;
                a.download = `proxies-${workspaceId}.csv`;
                a.click();
                URL.revokeObjectURL(url);
              },
            },
          ]}
          emptyMessage="No proxies are assigned to this workspace."
          defaultPageSize={10}
        />
      </Panel>
      {formOpen ? (
        <ProxyForm
          editing={editing}
          loading={mutation === "loading"}
          onCancel={() => {
            setEditing(undefined);
            setFormOpen(false);
            setError(undefined);
          }}
          onSubmit={(input) => void submit(input)}
          workspaceId={workspaceId}
        />
      ) : null}
      {message ? <Status label={message} tone="success" live /> : null}
      {error ? (
        <Panel
          title="Proxy mutation failed"
          description={error}
          actions={<Button onClick={() => setError(undefined)}>Dismiss</Button>}
        />
      ) : null}
      <Panel
        title="Profile assignment"
        description="New profiles can bind a proxy through the existing `profile.create` contract. Reassigning an existing profile requires a backend profile-update contract and is not exposed here."
        actions={
          <a
            href="/operations"
            style={{ color: colors.brand[700], fontWeight: 650 }}
          >
            Create profile with proxy
          </a>
        }
      />
    </div>
  );
}

function ProxyForm({
  editing,
  loading,
  onCancel,
  onSubmit,
  workspaceId,
}: {
  editing?: ProxyRecord;
  loading: boolean;
  onCancel: () => void;
  onSubmit: (input: ProxyCreateInput | ProxyUpdateInput) => void;
  workspaceId: string;
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
  const submit = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const numericPort = Number(port);
    if (
      !id.trim() ||
      !host.trim() ||
      !Number.isInteger(numericPort) ||
      numericPort < 1 ||
      numericPort > 65535
    ) {
      setValidation("Enter an ID, host, and port from 1 to 65535.");
      return;
    }
    const now = new Date().toISOString();
    const proxy: ProxyMetadataInput = {
      id: id.trim(),
      ...(name.trim() ? { name: name.trim() } : {}),
      protocol,
      host: host.trim(),
      port: numericPort,
      bypassRules: bypassRules
        .split("\n")
        .map((rule) => rule.trim())
        .filter(Boolean),
      createdAt: editing?.proxy.createdAt ?? now,
      updatedAt: now,
    };
    onSubmit(
      editing
        ? {
            workspaceId,
            proxyId: editing.proxy.id,
            expectedRevision: editing.revision,
            proxy,
          }
        : { workspaceId, proxy },
    );
  };
  return (
    <Panel
      title={editing ? "Edit proxy" : "Add proxy"}
      description="Only contract fields are editable. Credential material is never entered in this web form."
    >
      <form onSubmit={submit} style={{ display: "grid", gap: spacing[3] }}>
        <div
          style={{
            display: "grid",
            gap: spacing[3],
            gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))",
          }}
        >
          <Field
            label="ID"
            value={id}
            onChange={(event) => setId(event.currentTarget.value)}
            disabled={Boolean(editing)}
            required
          />
          <Field
            label="Name"
            value={name}
            onChange={(event) => setName(event.currentTarget.value)}
          />
          <label style={{ display: "grid", gap: spacing[1] }}>
            <span style={typography.label}>Protocol</span>
            <select
              aria-label="Protocol"
              value={protocol}
              onChange={(event) =>
                setProtocol(event.currentTarget.value as ProxyProtocol)
              }
              style={controlStyle}
            >
              {protocols.map((value) => (
                <option key={value}>{value}</option>
              ))}
            </select>
          </label>
          <Field
            label="Host"
            value={host}
            onChange={(event) => setHost(event.currentTarget.value)}
            required
          />
          <Field
            label="Port"
            inputMode="numeric"
            value={port}
            onChange={(event) => setPort(event.currentTarget.value)}
            required
          />
        </div>
        <Field
          label="Bypass rules"
          help="One rule per line."
          multiline
          value={bypassRules}
          onChange={(event) => setBypassRules(event.currentTarget.value)}
          error={validation}
        />
        <div style={{ display: "flex", gap: spacing[2] }}>
          <Button type="submit" variant="primary" loading={loading}>
            {editing ? "Save proxy" : "Create proxy"}
          </Button>
          <Button type="button" onClick={onCancel} disabled={loading}>
            Cancel
          </Button>
        </div>
      </form>
    </Panel>
  );
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
