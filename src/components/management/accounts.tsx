"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
  LuArrowRight,
  LuCheck,
  LuChevronDown,
  LuDownload,
  LuEllipsis,
  LuFileUp,
  LuKeyRound,
  LuPencil,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuShield,
  LuTrash2,
  LuUserPlus,
  LuX,
} from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { createTauriDesktopApi } from "../../../apps/desktop/src/bridge";
import { accountDirectoryStatuses } from "../../../apps/desktop/src/management-account-status";
import type {
  AccountIdentity,
  AccountProvisioningReceipt,
  AccountStatus,
  AccountView,
  GroupRecord,
  MemberRole,
  Organization,
  Workspace,
} from "../../../frontend/src/generated/command-registry";
import {
  type AccountImportRow,
  parseAccountTable,
  toAccountCsv,
  validateAccountRows,
} from "./account-input";
import "./accounts-i18n";

export const managementAccountApi = createTauriDesktopApi();
const api = managementAccountApi;
const roleOrder: MemberRole[] = [
  "owner",
  "admin",
  "lead",
  "operator",
  "member",
];
const td = "px-3 py-2.5 text-sm align-middle";
const th = "px-3 py-2 text-left text-xs font-medium text-muted-foreground";

export function accountError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "title" in error)
    return String(error.title);
  return String(error);
}

export function AccountError({
  message,
  onRetry,
}: {
  message: string;
  onRetry?: () => void;
}) {
  const { t } = useTranslation("managementAccounts");
  if (!message) return null;
  return (
    <div
      role="alert"
      className="flex items-start justify-between gap-3 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm text-destructive"
    >
      <span className="min-w-0 break-words">{message}</span>
      {onRetry ? (
        <Button size="sm" variant="outline" onClick={onRetry}>
          {t("retry")}
        </Button>
      ) : null}
    </div>
  );
}

export function RolePicker({
  value,
  onChange,
  disabled,
  includeOwner = false,
}: {
  value: MemberRole;
  onChange: (role: MemberRole) => void;
  disabled?: boolean;
  includeOwner?: boolean;
}) {
  const { t } = useTranslation("managementAccounts");
  return (
    <Select
      value={value}
      onValueChange={(next) => onChange(next as MemberRole)}
      disabled={disabled}
    >
      <SelectTrigger className="w-full min-w-36">
        <SelectValue />
      </SelectTrigger>
      <SelectContent className="max-w-[calc(100vw-2rem)]">
        {roleOrder
          .filter((role) => includeOwner || role !== "owner")
          .map((role) => (
            <SelectItem key={role} value={role} title={t(`${role}Help`)}>
              <span className="font-medium">{t(role)}</span>
            </SelectItem>
          ))}
      </SelectContent>
    </Select>
  );
}

function MultiScope({
  options,
  selected,
  onChange,
  label,
  disabled,
}: {
  options: { id: string; name: string }[];
  selected: string[];
  onChange: (items: string[]) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          className="w-full justify-between"
          disabled={disabled}
        >
          {selected.length
            ? `${selected.length} · ${options
                .filter((item) => selected.includes(item.id))
                .map((item) => item.name)
                .join(", ")}`
            : label}
          <LuChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        className="max-h-64 w-72 overflow-auto"
        align="start"
      >
        <DropdownMenuLabel>{label}</DropdownMenuLabel>
        {options.map((item) => (
          <DropdownMenuCheckboxItem
            key={item.id}
            checked={selected.includes(item.id)}
            onSelect={(event) => event.preventDefault()}
            onCheckedChange={(checked) =>
              onChange(
                checked
                  ? [...selected, item.id]
                  : selected.filter((id) => id !== item.id),
              )
            }
          >
            {item.name}
          </DropdownMenuCheckboxItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function WorkspaceTagPicker({
  options,
  selected,
  onChange,
  label,
  disabled,
  unavailableIds = [],
}: {
  options: { id: string; name: string }[];
  selected: string[];
  onChange: (items: string[]) => void;
  label: string;
  disabled?: boolean;
  unavailableIds?: string[];
}) {
  const { t } = useTranslation("managementAccounts");
  const [open, setOpen] = useState(false);
  const chosen = options.filter((item) => selected.includes(item.id));
  return (
    <div className="flex min-h-9 min-w-0 flex-wrap items-center gap-1.5 rounded-md border border-input bg-background px-2 py-1.5 focus-within:ring-2 focus-within:ring-ring">
      {chosen.map((item) => (
        <Badge
          key={item.id}
          variant="secondary"
          className="max-w-full gap-1 pr-1 font-normal"
        >
          <span className="truncate">{item.name}</span>
          {!disabled ? (
            <button
              type="button"
              aria-label={t("removeWorkspace", { name: item.name })}
              className="rounded-sm p-0.5 text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              onClick={() => onChange(selected.filter((id) => id !== item.id))}
            >
              <LuX className="size-3" />
            </button>
          ) : null}
        </Badge>
      ))}
      <Popover open={open} onOpenChange={disabled ? undefined : setOpen}>
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 shrink-0 gap-1 px-2 text-muted-foreground"
            disabled={disabled || options.length === 0}
            aria-expanded={open}
          >
            <LuPlus className="size-3.5" />
            {label}
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-72 p-0">
          <Command>
            <CommandInput placeholder={t("searchWorkspace")} />
            <CommandList>
              <CommandEmpty>{t("noResults")}</CommandEmpty>
              <CommandGroup>
                {options.map((item) => {
                  const isSelected = selected.includes(item.id);
                  const unavailable = unavailableIds.includes(item.id);
                  return (
                    <CommandItem
                      key={item.id}
                      value={`${item.name} ${item.id}`}
                      disabled={unavailable}
                      onSelect={() =>
                        onChange(
                          isSelected
                            ? selected.filter((id) => id !== item.id)
                            : [...selected, item.id],
                        )
                      }
                    >
                      <LuCheck
                        className={`size-4 shrink-0 ${isSelected ? "opacity-100" : "opacity-0"}`}
                      />
                      <span className="min-w-0 flex-1 truncate">
                        {item.name}
                      </span>
                      {unavailable ? (
                        <span className="text-xs text-muted-foreground">
                          {t("alreadyAssigned")}
                        </span>
                      ) : null}
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </div>
  );
}

async function downloadText(name: string, content: string) {
  const blob = new Blob(["\uFEFF", content], {
    type: "text/csv;charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

type AccountDraft = AccountImportRow & {
  accountId: string;
  key: string;
  workspaceIds: string[];
  role: MemberRole;
  groupIds: string[];
  receipt?: AccountProvisioningReceipt;
  outcome?: "succeeded" | "failed";
  message?: string;
};
type ProvisioningProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  organizationId?: string;
  workspaceId?: string;
  onCreated?: () => void;
  initialMode?: "single" | "bulk";
};

export function AccountProvisioning({
  open,
  onOpenChange,
  organizationId,
  workspaceId,
  onCreated,
  initialMode = "single",
}: ProvisioningProps) {
  const { t } = useTranslation("managementAccounts");
  const [mode, setMode] = useState<"single" | "bulk">(initialMode);
  const [stage, setStage] = useState(0);
  const [source, setSource] = useState<"auto" | "paste">("auto");
  const [organizations, setOrganizations] = useState<Organization[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [groupMap, setGroupMap] = useState<Record<string, GroupRecord[]>>({});
  const [orgId, setOrgId] = useState(organizationId ?? "");
  const [defaultWorkspaceIds, setDefaultWorkspaceIds] = useState<string[]>(
    workspaceId ? [workspaceId] : [],
  );
  const [commonRole, setCommonRole] = useState<MemberRole>("member");
  const [commonGroups, setCommonGroups] = useState<string[]>([]);
  const [rows, setRows] = useState<AccountDraft[]>([]);
  const [count, setCount] = useState(20);
  const [prefix, setPrefix] = useState("member-");
  const [start, setStart] = useState(1);
  const [template, setTemplate] = useState("{n}");
  const [paste, setPaste] = useState("");
  const [showPasswords, setShowPasswords] = useState(false);
  const [secretExport, setSecretExport] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const file = useRef<HTMLInputElement>(null);
  const batchKey = useRef("");
  const cancelGeneration = useRef(false);

  const issues = useMemo(() => validateAccountRows(rows), [rows]);
  const validCount = rows.filter(
    (row, index) => issues[index].length === 0 && row.workspaceIds.length > 0,
  ).length;
  const allGroups = useMemo(() => Object.values(groupMap).flat(), [groupMap]);
  const reset = useCallback(() => {
    setRows([]);
    setPaste("");
    setStage(0);
    setError("");
    setSecretExport(false);
    setSelected([]);
    setMode(initialMode);
    setShowPasswords(false);
    batchKey.current = `management-create-${crypto.randomUUID()}`;
  }, [initialMode]);

  useEffect(() => {
    if (!open) {
      cancelGeneration.current = true;
      setRows([]);
      setPaste("");
      return;
    }
    reset();
    cancelGeneration.current = false;
    setOrgId(organizationId ?? "");
    setDefaultWorkspaceIds(workspaceId ? [workspaceId] : []);
    let alive = true;
    api
      .listOrganizations()
      .then((page) => {
        if (alive) {
          setOrganizations(page.items);
          if (!organizationId && page.items.length === 1)
            setOrgId(page.items[0].id);
        }
      })
      .catch((reason) => {
        if (alive) setError(accountError(reason));
      });
    return () => {
      alive = false;
      cancelGeneration.current = true;
    };
  }, [open, reset, organizationId, workspaceId]);

  useEffect(() => {
    if (!open || !orgId) {
      setWorkspaces([]);
      return;
    }
    let alive = true;
    api
      .listWorkspaces(orgId)
      .then(async (page) => {
        if (!alive) return;
        setWorkspaces(page.items);
        if (!workspaceId && page.items.length === 1)
          setDefaultWorkspaceIds([page.items[0].id]);
        const results = await Promise.allSettled(
          page.items.map(
            async (item) => [item.id, await api.listGroups(item.id)] as const,
          ),
        );
        if (alive)
          setGroupMap(
            Object.fromEntries(
              results.flatMap((result) =>
                result.status === "fulfilled" ? [result.value] : [],
              ),
            ),
          );
      })
      .catch((reason) => {
        if (alive) setError(accountError(reason));
      });
    return () => {
      alive = false;
    };
  }, [open, orgId, workspaceId]);

  async function makeDrafts(number: number, imported?: AccountImportRow[]) {
    setPending(true);
    setError("");
    try {
      if (number < 1 || number > 100) throw new Error(t("maxRows"));
      const generated = await api.generateAccountDrafts({
        count: number,
        prefix,
        start,
        displayNameTemplate: template,
      });
      if (cancelGeneration.current) return;
      const drafts: AccountDraft[] = generated.items.map((item, index) => ({
        accountId: item.accountId,
        key: item.accountId,
        username: imported?.[index]?.username || item.username,
        displayName: imported?.[index]?.displayName || item.displayName,
        email: imported?.[index]?.email ?? "",
        initialPassword:
          imported?.[index]?.initialPassword || item.initialPassword,
        workspaceIds: [...defaultWorkspaceIds],
        role: commonRole,
        groupIds: [...commonGroups],
      }));
      setRows(drafts);
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }

  async function importRows(text: string) {
    try {
      const imported = parseAccountTable(text);
      if (!imported.length) throw new Error(t("emptyImport"));
      await makeDrafts(imported.length, imported);
      setPaste("");
    } catch (reason) {
      setError(
        reason instanceof Error && reason.message === "unclosedQuote"
          ? t("unclosedQuote")
          : accountError(reason),
      );
    }
  }

  async function addRow() {
    setPending(true);
    setError("");
    try {
      if (rows.length >= 100) throw new Error(t("maxRows"));
      const generated = await api.generateAccountDrafts({
        count: 1,
        prefix,
        start: start + rows.length,
        displayNameTemplate: template,
      });
      const item = generated.items[0];
      if (item && !cancelGeneration.current)
        setRows((items) => [
          ...items,
          {
            ...item,
            key: item.accountId,
            email: "",
            workspaceIds: [...defaultWorkspaceIds],
            role: commonRole,
            groupIds: [...commonGroups],
          },
        ]);
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }

  function editRow(key: string, change: Partial<AccountDraft>) {
    setRows((items) =>
      items.map((item) =>
        item.key === key
          ? { ...item, ...change, outcome: undefined, message: undefined }
          : item,
      ),
    );
    batchKey.current = `management-create-${crypto.randomUUID()}`;
  }

  async function execute(retryOnly = false) {
    setPending(true);
    setError("");
    const candidates = rows.filter(
      (row, index) =>
        issues[index].length === 0 &&
        row.workspaceIds.length > 0 &&
        (!retryOnly || row.outcome === "failed"),
    );
    let nextRows = [...rows];
    try {
      const creating = candidates.filter((row) => !row.receipt);
      if (creating.length) {
        const receipt = await api.managementBatch(
          {
            items: creating.map((row) => ({
              key: row.key,
              commandId: "account.create.scoped",
              input: {
                workspaceId: row.workspaceIds[0],
                accountId: row.accountId,
                username: row.username.trim(),
                displayName: row.displayName.trim(),
                email: row.email.trim(),
                role: row.role,
                initialPassword: row.initialPassword,
              },
            })),
          },
          retryOnly
            ? `${batchKey.current}-retry-${crypto.randomUUID()}`
            : batchKey.current,
        );
        nextRows = nextRows.map((row) => {
          const result = receipt.items.find((item) => item.key === row.key);
          if (!result) return row;
          return result.status === "succeeded"
            ? {
                ...row,
                receipt: result.data as AccountProvisioningReceipt,
                outcome: "succeeded" as const,
                message: t("accountCreated"),
              }
            : {
                ...row,
                outcome: "failed" as const,
                message: result.error?.title ?? t("failed"),
              };
        });
        setRows(nextRows);
      }
      const orgMembers = orgId
        ? await api.listOrganizationMembers(orgId)
        : undefined;
      const followUp: {
        key: string;
        commandId: string;
        input: Record<string, unknown>;
      }[] = [];
      for (const row of candidates) {
        const current = nextRows.find((item) => item.key === row.key);
        if (!current?.receipt) continue;
        const orgMember = orgMembers?.items.find(
          (item) => item.accountId === row.accountId,
        );
        for (const wsId of row.workspaceIds.slice(1)) {
          if (!orgMember) {
            nextRows = nextRows.map((item) =>
              item.key === row.key
                ? { ...item, outcome: "failed", message: t("accessFailed") }
                : item,
            );
            continue;
          }
          followUp.push({
            key: `${row.key}:workspace:${wsId}`,
            commandId: "organization.member.workspace.assign",
            input: {
              organizationId: orgId,
              workspaceId: wsId,
              organizationMemberId: orgMember.id,
              organizationMemberRevision: orgMember.revision,
              role: row.role,
            },
          });
        }
      }
      for (let offset = 0; offset < followUp.length; offset += 100) {
        const receipt = await api.managementBatch(
          { items: followUp.slice(offset, offset + 100) },
          `${batchKey.current}-access-${offset}-${retryOnly ? crypto.randomUUID() : "first"}`,
        );
        for (const result of receipt.items)
          if (result.status === "failed")
            nextRows = nextRows.map((row) =>
              result.key.startsWith(`${row.key}:`)
                ? {
                    ...row,
                    outcome: "failed",
                    message: `${t("accessFailed")}: ${result.error?.title ?? t("failed")}`,
                  }
                : row,
            );
      }
      const grantItems: {
        key: string;
        commandId: string;
        input: Record<string, unknown>;
      }[] = [];
      for (const row of candidates) {
        const current = nextRows.find((item) => item.key === row.key);
        if (!current?.receipt) continue;
        for (const wsId of row.workspaceIds) {
          const scopedGroups = (groupMap[wsId] ?? []).filter((group) =>
            row.groupIds.includes(group.group.id),
          );
          if (!scopedGroups.length) continue;
          const member = (await api.listMembers(wsId)).items.find(
            (item) => item.accountId === row.accountId,
          );
          if (!member) {
            nextRows = nextRows.map((item) =>
              item.key === row.key
                ? { ...item, outcome: "failed", message: t("accessFailed") }
                : item,
            );
            continue;
          }
          for (const group of scopedGroups)
            for (const action of ["read", "execute"])
              grantItems.push({
                key: `${row.key}:group:${group.group.id}:${action}`,
                commandId: "group.share.create",
                input: {
                  workspaceId: wsId,
                  groupId: group.group.id,
                  memberId: member.id,
                  action,
                  effect: "allow",
                },
              });
        }
      }
      for (let offset = 0; offset < grantItems.length; offset += 100) {
        const receipt = await api.managementBatch(
          { items: grantItems.slice(offset, offset + 100) },
          `${batchKey.current}-groups-${offset}-${retryOnly ? crypto.randomUUID() : "first"}`,
        );
        for (const result of receipt.items)
          if (result.status === "failed")
            nextRows = nextRows.map((row) =>
              result.key.startsWith(`${row.key}:`)
                ? {
                    ...row,
                    outcome: "failed",
                    message: `${t("accessFailed")}: ${result.error?.title ?? t("failed")}`,
                  }
                : row,
            );
      }
      setRows(nextRows);
      setStage(3);
      onCreated?.();
    } catch (reason) {
      setError(accountError(reason));
      setRows(
        nextRows.map((row) =>
          candidates.some((candidate) => candidate.key === row.key) &&
          row.outcome !== "failed"
            ? { ...row, outcome: "failed", message: accountError(reason) }
            : row,
        ),
      );
      setStage(3);
    } finally {
      setPending(false);
    }
  }

  const scopedGroups = allGroups
    .filter((group) => defaultWorkspaceIds.includes(group.workspaceId))
    .map((group) => ({ id: group.group.id, name: group.group.name }));
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!pending) onOpenChange(next);
      }}
    >
      <DialogContent
        className={
          mode === "bulk" || stage > 0
            ? "max-w-6xl gap-0 overflow-hidden p-0"
            : "max-w-2xl gap-0 overflow-hidden p-0"
        }
        dismissible={!pending}
      >
        <DialogHeader className="border-b border-border px-5 py-4">
          <DialogTitle>
            {mode === "single" ? t("createAccount") : t("createMany")}
          </DialogTitle>
          <DialogDescription>
            {organizations.find((org) => org.id === orgId)?.name ??
              t("requiresScope")}
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-wrap items-center gap-3 border-b border-border px-5 py-2.5">
          {["prepare", "permissions", "review", "results"].map((key, index) => (
            <span
              key={key}
              className={`flex items-center gap-2 text-xs ${index === stage ? "font-semibold text-foreground" : "text-muted-foreground"}`}
            >
              <span
                className={`flex size-5 items-center justify-center rounded-full ${index < stage ? "bg-primary text-primary-foreground" : "bg-muted"}`}
              >
                {index < stage ? <LuCheck className="size-3" /> : index + 1}
              </span>
              {t(key)}
              {index < 3 ? <LuArrowRight className="size-3" /> : null}
            </span>
          ))}
        </div>
        <div className="max-h-[65vh] space-y-4 overflow-auto p-5">
          <AccountError message={error} />
          {stage === 0 ? (
            <>
              <div className="flex items-center gap-2">
                <Button
                  variant={mode === "single" ? "secondary" : "ghost"}
                  size="sm"
                  onClick={() => setMode("single")}
                  disabled={pending}
                >
                  {t("single")}
                </Button>
                <Button
                  variant={mode === "bulk" ? "secondary" : "ghost"}
                  size="sm"
                  onClick={() => setMode("bulk")}
                  disabled={pending}
                >
                  {t("bulk")}
                </Button>
              </div>
              {!organizationId ? (
                <div className="grid gap-1.5">
                  <Label>{t("organization")}</Label>
                  <Select
                    value={orgId}
                    onValueChange={(value) => {
                      setOrgId(value);
                      setDefaultWorkspaceIds([]);
                      setRows([]);
                    }}
                  >
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder={t("selectOrganization")} />
                    </SelectTrigger>
                    <SelectContent>
                      {organizations.map((org) => (
                        <SelectItem key={org.id} value={org.id}>
                          {org.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              ) : null}
              {mode === "bulk" ? (
                <>
                  <div className="flex flex-wrap items-center gap-2">
                    <Button
                      variant={source === "auto" ? "secondary" : "outline"}
                      size="sm"
                      onClick={() => setSource("auto")}
                    >
                      <LuKeyRound />
                      {t("auto")}
                    </Button>
                    <Button
                      variant={source === "paste" ? "secondary" : "outline"}
                      size="sm"
                      onClick={() => setSource("paste")}
                    >
                      <LuFileUp />
                      {t("paste")}
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => file.current?.click()}
                    >
                      <LuFileUp />
                      {t("csv")}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() =>
                        void downloadText(
                          "accounts-template.csv",
                          "username,displayName,email,initialPassword\r\n",
                        )
                      }
                    >
                      <LuDownload />
                      {t("sample")}
                    </Button>
                    <input
                      ref={file}
                      type="file"
                      className="hidden"
                      accept=".csv,.tsv,text/csv,text/tab-separated-values"
                      onChange={async (event) => {
                        const chosen = event.target.files?.[0];
                        if (chosen) {
                          try {
                            await importRows(await chosen.text());
                          } catch (reason) {
                            setError(accountError(reason));
                          }
                        }
                        event.target.value = "";
                      }}
                    />
                  </div>
                  {source === "auto" ? (
                    <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
                      <div className="space-y-1.5">
                        <Label>{t("count")}</Label>
                        <Input
                          type="number"
                          min={1}
                          max={100}
                          value={count}
                          onChange={(event) =>
                            setCount(Number(event.target.value))
                          }
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label>{t("prefix")}</Label>
                        <Input
                          value={prefix}
                          onChange={(event) => setPrefix(event.target.value)}
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label>{t("start")}</Label>
                        <Input
                          type="number"
                          min={0}
                          value={start}
                          onChange={(event) =>
                            setStart(Number(event.target.value))
                          }
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label>{t("nameTemplate")}</Label>
                        <Input
                          value={template}
                          onChange={(event) => setTemplate(event.target.value)}
                        />
                      </div>
                      <div className="col-span-2 flex items-center justify-between gap-3 lg:col-span-4">
                        <span className="text-xs text-muted-foreground">
                          {t("generateHint")}
                        </span>
                        <Button
                          size="sm"
                          disabled={pending}
                          onClick={() => void makeDrafts(count)}
                        >
                          {pending ? t("processing") : t("generateRows")}
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <div className="space-y-2">
                      <Label>{t("importHint")}</Label>
                      <textarea
                        className="min-h-32 w-full rounded-md border border-input bg-background p-3 font-mono text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        value={paste}
                        onChange={(event) => setPaste(event.target.value)}
                      />
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={pending || !paste.trim()}
                        onClick={() => void importRows(paste)}
                      >
                        {t("import")}
                      </Button>
                    </div>
                  )}
                </>
              ) : rows.length === 0 ? (
                <Button
                  variant="outline"
                  disabled={pending}
                  onClick={() => void makeDrafts(1)}
                >
                  <LuKeyRound />
                  {pending ? t("processing") : t("generate")}
                </Button>
              ) : null}
              {rows.length > 0 ? (
                <>
                  <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
                    <span>{t("rows", { count: rows.length })}</span>
                    <div className="flex items-center gap-2">
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => setShowPasswords((show) => !show)}
                      >
                        {showPasswords ? t("hide") : t("show")}
                      </Button>
                      {mode === "bulk" ? (
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => void addRow()}
                          disabled={pending}
                        >
                          <LuPlus />
                          {t("addRow")}
                        </Button>
                      ) : null}
                    </div>
                  </div>
                  <div className="overflow-auto rounded-lg border border-border">
                    <table className="w-full table-fixed">
                      <thead className="bg-muted/50">
                        <tr>
                          {mode === "bulk" ? (
                            <th className="w-10 px-3">
                              <Checkbox
                                aria-label={t("selectAll")}
                                checked={selected.length === rows.length}
                                onCheckedChange={(checked) =>
                                  setSelected(
                                    checked ? rows.map((row) => row.key) : [],
                                  )
                                }
                              />
                            </th>
                          ) : null}
                          {["username", "displayName", "email", "password"].map(
                            (key) => (
                              <th key={key} className={th}>
                                {t(key)}
                              </th>
                            ),
                          )}
                          {mode === "bulk" ? <th className="w-10" /> : null}
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-border">
                        {rows.map((row, index) => (
                          <tr key={row.key}>
                            {mode === "bulk" ? (
                              <td className="px-3">
                                <Checkbox
                                  aria-label={`${t("select")} ${row.username}`}
                                  checked={selected.includes(row.key)}
                                  onCheckedChange={(checked) =>
                                    setSelected((items) =>
                                      checked
                                        ? [...items, row.key]
                                        : items.filter(
                                            (key) => key !== row.key,
                                          ),
                                    )
                                  }
                                />
                              </td>
                            ) : null}
                            {(
                              [
                                "username",
                                "displayName",
                                "email",
                                "initialPassword",
                              ] as const
                            ).map((field) => (
                              <td key={field} className="p-1.5">
                                <Input
                                  className="h-8"
                                  aria-label={`${t(field === "initialPassword" ? "password" : field)} ${index + 1}`}
                                  value={row[field]}
                                  type={
                                    field === "initialPassword" &&
                                    !showPasswords
                                      ? "password"
                                      : "text"
                                  }
                                  onChange={(event) =>
                                    editRow(row.key, {
                                      [field]: event.target.value,
                                    })
                                  }
                                />
                              </td>
                            ))}
                            {mode === "bulk" ? (
                              <td>
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  className="size-7"
                                  aria-label={t("remove")}
                                  onClick={() =>
                                    setRows((items) =>
                                      items.filter(
                                        (item) => item.key !== row.key,
                                      ),
                                    )
                                  }
                                >
                                  <LuX className="size-3" />
                                </Button>
                              </td>
                            ) : null}
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                  {selected.length ? (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        setRows((items) =>
                          items.filter((item) => !selected.includes(item.key)),
                        );
                        setSelected([]);
                      }}
                    >
                      <LuTrash2 />
                      {t("removeSelected")}
                    </Button>
                  ) : null}
                </>
              ) : null}
            </>
          ) : null}
          {stage === 1 ? (
            <>
              <p className="text-sm text-muted-foreground">{t("scopeHint")}</p>
              <div className="grid gap-3 rounded-lg border border-border p-3 lg:grid-cols-4">
                <WorkspaceTagPicker
                  label={t("selectWorkspace")}
                  options={workspaces}
                  selected={defaultWorkspaceIds}
                  onChange={setDefaultWorkspaceIds}
                />
                <RolePicker value={commonRole} onChange={setCommonRole} />
                <MultiScope
                  label={t("groups")}
                  options={scopedGroups}
                  selected={commonGroups}
                  onChange={setCommonGroups}
                />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    setRows((items) =>
                      items.map((item) => ({
                        ...item,
                        workspaceIds: [...defaultWorkspaceIds],
                        role: commonRole,
                        groupIds: [...commonGroups],
                      })),
                    )
                  }
                >
                  {t("applyAll")}
                </Button>
              </div>
              <div className="overflow-auto rounded-lg border border-border">
                <table className="w-full table-fixed">
                  <thead className="bg-muted/50">
                    <tr>
                      {["username", "workspaces", "role", "groups"].map(
                        (key) => (
                          <th key={key} className={th}>
                            {t(key)}
                          </th>
                        ),
                      )}
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border">
                    {rows.map((row) => (
                      <tr key={row.key}>
                        <td className={td}>
                          <span className="block truncate font-medium">
                            {row.displayName || row.username}
                          </span>
                          <span className="text-xs text-muted-foreground">
                            {row.username}
                          </span>
                        </td>
                        <td className={td}>
                          <WorkspaceTagPicker
                            label={t("selectWorkspace")}
                            options={workspaces}
                            selected={row.workspaceIds}
                            onChange={(items) =>
                              editRow(row.key, {
                                workspaceIds: items,
                                groupIds: row.groupIds.filter((id) =>
                                  allGroups.some(
                                    (group) =>
                                      group.group.id === id &&
                                      items.includes(group.workspaceId),
                                  ),
                                ),
                              })
                            }
                          />
                        </td>
                        <td className={td}>
                          <RolePicker
                            value={row.role}
                            onChange={(value) =>
                              editRow(row.key, { role: value })
                            }
                          />
                        </td>
                        <td className={td}>
                          <MultiScope
                            label={t("groups")}
                            options={allGroups
                              .filter((group) =>
                                row.workspaceIds.includes(group.workspaceId),
                              )
                              .map((group) => ({
                                id: group.group.id,
                                name: group.group.name,
                              }))}
                            selected={row.groupIds}
                            onChange={(items) =>
                              editRow(row.key, { groupIds: items })
                            }
                          />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="text-xs text-muted-foreground">
                {t("groupHint")} {t(`${commonRole}Help`)} {t("roleHint")}
              </p>
            </>
          ) : null}
          {stage === 2 ? (
            <>
              <p className="text-sm text-muted-foreground">
                {t("validationHint")}
              </p>
              <div className="overflow-hidden rounded-lg border border-border">
                <table className="w-full">
                  <thead className="bg-muted/50">
                    <tr>
                      {["username", "workspaces", "role", "reason"].map(
                        (key) => (
                          <th key={key} className={th}>
                            {t(key)}
                          </th>
                        ),
                      )}
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border">
                    {rows.map((row, index) => (
                      <tr key={row.key}>
                        <td className={td}>{row.username}</td>
                        <td className={td}>
                          {workspaces
                            .filter((item) =>
                              row.workspaceIds.includes(item.id),
                            )
                            .map((item) => item.name)
                            .join(", ") || t("noWorkspace")}
                        </td>
                        <td className={td}>{t(row.role)}</td>
                        <td className={td}>
                          {issues[index].length || !row.workspaceIds.length ? (
                            <span className="text-xs text-destructive">
                              {[
                                ...issues[index].map((issue) =>
                                  t(`issue_${issue}`),
                                ),
                                ...(!row.workspaceIds.length
                                  ? [t("noWorkspace")]
                                  : []),
                              ].join(" ")}
                            </span>
                          ) : (
                            <Badge variant="secondary">
                              <LuCheck className="mr-1 size-3" />
                              {t("valid")}
                            </Badge>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          ) : null}
          {stage === 3 ? (
            <>
              <div className="flex items-center justify-between gap-3">
                <span className="text-sm font-medium">
                  {t("resultSummary", {
                    success: rows.filter((row) => row.outcome === "succeeded")
                      .length,
                    failed: rows.filter((row) => row.outcome === "failed")
                      .length,
                  })}
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={
                    pending || !rows.some((row) => row.outcome === "failed")
                  }
                  onClick={() => void execute(true)}
                >
                  <LuRefreshCw />
                  {t("retryFailed")}
                </Button>
              </div>
              <div className="overflow-auto rounded-lg border border-border">
                <table className="w-full">
                  <thead className="bg-muted/50">
                    <tr>
                      {["username", "identity", "reason"].map((key) => (
                        <th className={th} key={key}>
                          {t(key)}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border">
                    {rows.map((row) => (
                      <tr key={row.key}>
                        <td className={td}>{row.username}</td>
                        <td className={td}>
                          {t(row.receipt ? "created" : "pending")}
                        </td>
                        <td
                          className={`${td} ${row.outcome === "failed" ? "text-destructive" : "text-muted-foreground"}`}
                        >
                          {row.message ?? t("pending")}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="space-y-3 rounded-lg border border-border p-3">
                <p className="text-xs text-muted-foreground">
                  {t("secretNotice")}
                </p>
                <div className="flex items-center gap-2 text-xs">
                  <Checkbox
                    id="secret-export-confirm"
                    aria-label={t("confirmSecretExport")}
                    checked={secretExport}
                    onCheckedChange={(checked) =>
                      setSecretExport(checked === true)
                    }
                  />
                  <Label
                    htmlFor="secret-export-confirm"
                    className="cursor-pointer"
                  >
                    {t("confirmSecretExport")}
                  </Label>
                </div>
                <div className="flex gap-2">
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={!secretExport || !rows.some((row) => row.receipt)}
                    onClick={() =>
                      void downloadText(
                        "account-credentials.csv",
                        toAccountCsv(
                          rows.filter((row) => row.receipt),
                          true,
                        ),
                      )
                    }
                  >
                    <LuDownload />
                    {t("exportCredentials")}
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() =>
                      void downloadText(
                        "account-results.csv",
                        [
                          "username,status,result",
                          ...rows.map((row) =>
                            [
                              row.username,
                              row.outcome ?? "pending",
                              row.message ?? "",
                            ]
                              .map((cell) => `"${cell.replaceAll('"', '""')}"`)
                              .join(","),
                          ),
                        ].join("\r\n"),
                      )
                    }
                  >
                    {t("exportReport")}
                  </Button>
                </div>
              </div>
            </>
          ) : null}
        </div>
        <DialogFooter className="justify-between border-t border-border px-5 py-3">
          <Button
            variant="outline"
            onClick={() =>
              stage === 0 || stage === 3
                ? onOpenChange(false)
                : setStage((value) => value - 1)
            }
            disabled={pending}
          >
            {stage === 0 ? (
              t("cancel")
            ) : stage === 3 ? (
              t("close")
            ) : (
              <>
                <LuArrowLeft />
                {t("back")}
              </>
            )}
          </Button>
          {stage < 3 ? (
            <Button
              disabled={
                pending ||
                rows.length === 0 ||
                (stage === 2 && validCount === 0)
              }
              onClick={() =>
                stage === 2 ? void execute() : setStage((value) => value + 1)
              }
            >
              {pending ? (
                t("processing")
              ) : stage === 2 ? (
                t("createValid", { count: validCount })
              ) : (
                <>
                  {t("next")}
                  <LuArrowRight />
                </>
              )}
            </Button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

type DirectoryEntry = {
  accountId: string;
  username: string;
  displayName: string;
  email: string;
  status?: string;
  orgStatus?: string;
  workspaceStatus?: string;
  identity?: AccountIdentity;
  accountRevision?: number;
  workspaceIds: string[];
  workspaceNames: string[];
  orgMemberId?: string;
  orgRevision?: number;
};

export function AccountDirectory({
  organizationId,
  organizationOwnerId,
  workspaceId,
  canManageOrganization = false,
  onDataChanged,
}: {
  organizationId?: string;
  organizationOwnerId?: string;
  workspaceId?: string;
  canManageOrganization?: boolean;
  onDataChanged?: () => void;
}) {
  const { t } = useTranslation("managementAccounts");
  const [entries, setEntries] = useState<DirectoryEntry[]>([]);
  const [viewer, setViewer] = useState<AccountView>();
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [provisioning, setProvisioning] = useState<"single" | "bulk">();
  const [editing, setEditing] = useState<DirectoryEntry[]>([]);
  const [passwordTarget, setPasswordTarget] = useState<DirectoryEntry>();
  const [resetPassword, setResetPassword] = useState("");
  const [resetPasswordConfirm, setResetPasswordConfirm] = useState("");
  const [resetPasswordError, setResetPasswordError] = useState("");
  const [editValues, setEditValues] = useState<
    Record<
      string,
      {
        username: string;
        displayName: string;
        email: string;
        currentPassword?: string;
      }
    >
  >({});
  const [editResults, setEditResults] = useState<Record<string, string>>({});
  const [assigning, setAssigning] = useState(false);
  const [assignWorkspaceIds, setAssignWorkspaceIds] = useState<string[]>([]);
  const [assignRole, setAssignRole] = useState<MemberRole>("member");
  const [status, setStatus] = useState<AccountStatus>();
  const [orgStatus, setOrgStatus] = useState<"active" | "suspended">();
  const [outcomes, setOutcomes] = useState<
    { label: string; result: string; failed: boolean }[]
  >([]);
  const [statusRetry, setStatusRetry] = useState<
    | {
        scope: "account";
        status: AccountStatus;
        accountIds: string[];
      }
    | {
        scope: "organization";
        status: "active" | "suspended";
        accountIds: string[];
      }
  >();
  const reload = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const actor = await api.getAccount();
      setViewer(actor);
      const map = new Map<string, DirectoryEntry>();
      if (actor.platformAdmin) {
        const page = await api.listAccounts();
        for (const item of page.items)
          map.set(item.account.id, {
            accountId: item.account.id,
            username: item.identity?.username ?? "",
            displayName: item.identity?.displayName ?? "",
            email: item.identity?.email ?? "",
            status: item.account.status,
            accountRevision: item.account.revision,
            identity: item.identity ?? undefined,
            workspaceIds: [],
            workspaceNames: [],
          });
      }
      if (organizationId) {
        const [orgPage, wsPage] = await Promise.all([
          api.listOrganizationMembers(organizationId),
          api.listWorkspaces(organizationId),
        ]);
        setWorkspaces(wsPage.items);
        const accountIds = new Set(orgPage.items.map((item) => item.accountId));
        if (actor.platformAdmin)
          for (const key of map.keys())
            if (!accountIds.has(key)) map.delete(key);
        for (const item of orgPage.items)
          map.set(item.accountId, {
            ...map.get(item.accountId),
            accountId: item.accountId,
            username: item.username,
            displayName: map.get(item.accountId)?.displayName ?? "",
            email: item.email,
            status: map.get(item.accountId)?.status,
            orgStatus: item.status,
            workspaceIds: [],
            workspaceNames: [],
            orgMemberId: item.id,
            orgRevision: item.revision,
          });
        const memberships = await Promise.allSettled(
          wsPage.items.map(async (ws) => ({
            workspace: ws,
            members: (await api.listMembers(ws.id)).items,
          })),
        );
        if (memberships.some((result) => result.status === "rejected"))
          setError(t("partial"));
        for (const result of memberships)
          if (result.status === "fulfilled")
            for (const member of result.value.members) {
              const existing = map.get(member.accountId);
              if (existing) {
                existing.workspaceIds.push(result.value.workspace.id);
                existing.workspaceNames.push(result.value.workspace.name);
              } else
                map.set(member.accountId, {
                  accountId: member.accountId,
                  username: member.identity?.username ?? "",
                  displayName: "",
                  email: member.identity?.email ?? "",
                  workspaceStatus: member.status,
                  workspaceIds: [result.value.workspace.id],
                  workspaceNames: [result.value.workspace.name],
                });
            }
      } else if (workspaceId) {
        const page = await api.listMembers(workspaceId);
        const accountIds = new Set(page.items.map((item) => item.accountId));
        if (actor.platformAdmin)
          for (const key of map.keys())
            if (!accountIds.has(key)) map.delete(key);
        for (const member of page.items)
          if (!map.has(member.accountId))
            map.set(member.accountId, {
              accountId: member.accountId,
              username: member.identity?.username ?? "",
              displayName: "",
              email: member.identity?.email ?? "",
              workspaceStatus: member.status,
              workspaceIds: [workspaceId],
              workspaceNames: [],
            });
      }
      setEntries([...map.values()]);
      setSelected([]);
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setLoading(false);
    }
  }, [organizationId, workspaceId, t]);
  useEffect(() => {
    void reload();
  }, [reload]);
  const visible = entries.filter((entry) =>
    `${entry.username} ${entry.email} ${entry.displayName}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );
  const selectedEntries = entries.filter((entry) =>
    selected.includes(entry.accountId),
  );
  const assignableWorkspaces = workspaces.filter(
    (workspace) =>
      viewer?.platformAdmin ||
      viewer?.manageableWorkspaceIds.includes(workspace.id),
  );
  const unavailableWorkspaceIds = assignableWorkspaces
    .filter((workspace) =>
      selectedEntries.every((entry) =>
        entry.workspaceIds.includes(workspace.id),
      ),
    )
    .map((workspace) => workspace.id);
  const editable = (entry: DirectoryEntry) =>
    viewer?.platformAdmin || viewer?.accountId === entry.accountId;
  const canAssignWorkspace = (entry: DirectoryEntry) =>
    Boolean(
      organizationId &&
        entry.orgMemberId &&
        (viewer?.platformAdmin ||
          (organizationId &&
            viewer?.manageableOrganizationIds.includes(organizationId)) ||
          workspaces.some(
            (item) =>
              item.organizationId === organizationId &&
              viewer?.manageableWorkspaceIds.includes(item.id),
          )),
    );
  function beginEdit(items: DirectoryEntry[]) {
    setEditing(items);
    setEditValues(
      Object.fromEntries(
        items.map((item) => [
          item.accountId,
          {
            username: item.username,
            displayName: item.displayName,
            email: item.email,
          },
        ]),
      ),
    );
    setEditResults({});
  }
  async function saveEdits() {
    setPending(true);
    setError("");
    try {
      const changes = editing.filter((item) => editable(item));
      const identities = await Promise.all(
        changes.map(async (item) => ({
          item,
          identity:
            item.identity ??
            (await api.getAccountIdentity({ accountId: item.accountId })),
        })),
      );
      const result = await api.managementBatch(
        {
          items: identities.map(({ item, identity }) => {
            const values = editValues[item.accountId];
            const self = viewer?.accountId === item.accountId;
            return {
              key: item.accountId,
              commandId: "account.identity.update",
              input: {
                accountId: item.accountId,
                expectedRevision: identity.revision,
                username: values.username,
                displayName: values.displayName,
                email: values.email,
                ...(self ? { currentPassword: values.currentPassword } : {}),
              },
            };
          }),
        },
        `account-edit-${crypto.randomUUID()}`,
      );
      setEditResults(
        Object.fromEntries(
          result.items.map((item) => [
            item.key,
            item.status === "succeeded"
              ? t("changed")
              : (item.error?.title ?? t("failed")),
          ]),
        ),
      );
      await reload();
      onDataChanged?.();
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }
  async function changeStatus() {
    if (!status) return;
    const requestedStatus = status;
    setStatusRetry(undefined);
    setPending(true);
    setError("");
    try {
      const result = await api.managementBatch(
        {
          items: selectedEntries
            .filter((entry) => entry.accountRevision !== undefined)
            .map((entry) => ({
              key: entry.accountId,
              commandId: "account.status.update",
              input: {
                accountId: entry.accountId,
                expectedRevision: entry.accountRevision,
                status,
              },
            })),
        },
        `account-status-${crypto.randomUUID()}`,
      );
      setOutcomes(
        result.items.map((item) => ({
          label:
            entries.find((entry) => entry.accountId === item.key)?.username ||
            t("accounts"),
          result:
            item.status === "succeeded"
              ? t("success")
              : (item.error?.title ?? t("failed")),
          failed: item.status === "failed",
        })),
      );
      const failedIds = result.items
        .filter((item) => item.status === "failed")
        .map((item) => item.key);
      setStatusRetry(
        failedIds.length
          ? { scope: "account", status: requestedStatus, accountIds: failedIds }
          : undefined,
      );
      setStatus(undefined);
      await reload();
      onDataChanged?.();
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }
  async function resetAccountPassword() {
    if (!passwordTarget || !viewer?.platformAdmin) return;
    if (resetPassword.length < 12 || resetPassword.length > 256) {
      setResetPasswordError(t("adminPasswordLength"));
      return;
    }
    if (resetPassword !== resetPasswordConfirm) {
      setResetPasswordError(t("passwordMismatch"));
      return;
    }
    setPending(true);
    setResetPasswordError("");
    try {
      const result = await api.managementBatch(
        {
          items: [
            {
              key: passwordTarget.accountId,
              commandId: "account.password.change",
              input: {
                accountId: passwordTarget.accountId,
                newPassword: resetPassword,
              },
            },
          ],
        },
        `account-password-reset-${crypto.randomUUID()}`,
      );
      const item = result.items[0];
      if (item?.status !== "succeeded") {
        setResetPasswordError(item?.error?.title ?? t("failed"));
        return;
      }
      setOutcomes([
        {
          label: passwordTarget.username || passwordTarget.email,
          result: t("adminPasswordResetDone"),
          failed: false,
        },
      ]);
      setPasswordTarget(undefined);
      setResetPassword("");
      setResetPasswordConfirm("");
      await reload();
      onDataChanged?.();
    } catch (reason) {
      setResetPasswordError(accountError(reason));
    } finally {
      setPending(false);
    }
  }
  async function changeOrganizationStatus() {
    if (!organizationId || !orgStatus) return;
    const requestedStatus = orgStatus;
    setStatusRetry(undefined);
    setPending(true);
    setError("");
    try {
      const targets = selectedEntries.filter(
        (entry) => entry.orgMemberId && entry.orgRevision !== undefined,
      );
      const result = await api.managementBatch(
        {
          items: targets.map((entry) => ({
            key: entry.accountId,
            commandId: "organization.member.status.update",
            input: {
              organizationId,
              accountId: entry.accountId,
              status: orgStatus,
              expectedRevision: entry.orgRevision,
            },
          })),
        },
        `org-member-status-${crypto.randomUUID()}`,
      );
      setOutcomes(
        result.items.map((item) => ({
          label:
            entries.find((entry) => entry.accountId === item.key)?.username ||
            t("accounts"),
          result:
            item.status === "succeeded"
              ? t("success")
              : (item.error?.title ?? t("failed")),
          failed: item.status === "failed",
        })),
      );
      const failedIds = result.items
        .filter((item) => item.status === "failed")
        .map((item) => item.key);
      setStatusRetry(
        failedIds.length
          ? {
              scope: "organization",
              status: requestedStatus,
              accountIds: failedIds,
            }
          : undefined,
      );
      setOrgStatus(undefined);
      await reload();
      onDataChanged?.();
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }
  async function assign() {
    const targets = selectedEntries.flatMap((entry) =>
      assignWorkspaceIds
        .filter((id) => !entry.workspaceIds.includes(id))
        .map((workspaceId) => ({ entry, workspaceId })),
    );
    if (!targets.length) {
      setError(t("allWorkspacesAssigned"));
      return;
    }
    setPending(true);
    setError("");
    try {
      const result = await api.managementBatch(
        {
          items: targets
            .filter(
              ({ entry }) =>
                entry.orgMemberId && entry.orgRevision !== undefined,
            )
            .map(({ entry, workspaceId }) => ({
              key: `${entry.accountId}:${workspaceId}`,
              commandId: "organization.member.workspace.assign",
              input: {
                organizationId,
                workspaceId,
                organizationMemberId: entry.orgMemberId,
                organizationMemberRevision: entry.orgRevision,
                role: assignRole,
              },
            })),
        },
        `account-assign-${crypto.randomUUID()}`,
      );
      setStatusRetry(undefined);
      setOutcomes(
        result.items.map((item) => ({
          label: (() => {
            const target = targets.find(
              ({ entry, workspaceId }) =>
                `${entry.accountId}:${workspaceId}` === item.key,
            );
            const accountName =
              target?.entry.displayName ||
              target?.entry.username ||
              target?.entry.email ||
              t("accounts");
            const workspaceName = workspaces.find(
              (workspace) => workspace.id === target?.workspaceId,
            )?.name;
            return workspaceName
              ? `${accountName} · ${workspaceName}`
              : accountName;
          })(),
          result:
            item.status === "succeeded"
              ? t("success")
              : (item.error?.title ?? t("failed")),
          failed: item.status === "failed",
        })),
      );
      setAssigning(false);
      await reload();
      onDataChanged?.();
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setPending(false);
    }
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 p-4">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold">{t("directory")}</h2>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {t("directoryHint")}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="icon"
            aria-label={t("refresh")}
            disabled={loading}
            onClick={() => void reload()}
          >
            <LuRefreshCw className={loading ? "animate-spin" : ""} />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm">
                <LuPlus />
                {t("createAccount")}
                <LuChevronDown />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => setProvisioning("single")}>
                <LuUserPlus />
                {t("single")}
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => setProvisioning("bulk")}>
                <LuUsersIcon />
                {t("createMany")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <AccountError message={error} onRetry={() => void reload()} />
      <div className="relative max-w-md">
        <LuSearch className="absolute left-3 top-2.5 size-4 text-muted-foreground" />
        <Input
          className="pl-9"
          placeholder={t("search")}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setSelected([]);
          }}
        />
      </div>
      {selected.length ? (
        <div className="flex flex-wrap items-center gap-2 rounded-md border border-border bg-muted/30 px-3 py-2">
          <span className="mr-2 text-xs font-medium">
            {t("selected", { count: selected.length })}
          </span>
          {selectedEntries.every(editable) ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => beginEdit(selectedEntries)}
            >
              <LuPencil />
              {t("editMany")}
            </Button>
          ) : null}
          {selectedEntries.every(canAssignWorkspace) ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setAssignWorkspaceIds([]);
                setAssigning(true);
              }}
            >
              {t("assignWorkspace")}
            </Button>
          ) : null}
          {organizationId &&
          canManageOrganization &&
          selectedEntries.every((entry) =>
            Boolean(
              entry.orgMemberId &&
                entry.orgRevision !== undefined &&
                entry.accountId !== organizationOwnerId,
            ),
          ) ? (
            <>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setOrgStatus("suspended")}
              >
                {t("orgSuspend")}
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setOrgStatus("active")}
              >
                {t("orgRestore")}
              </Button>
            </>
          ) : null}
          {!organizationId && viewer?.platformAdmin ? (
            <>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setStatus("suspended")}
              >
                {t("suspend")}
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setStatus("active")}
              >
                {t("activate")}
              </Button>
            </>
          ) : null}
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            onClick={() => setSelected([])}
          >
            <LuX />
            {t("clearSelection")}
          </Button>
        </div>
      ) : null}
      <div className="min-h-0 flex-1 overflow-auto rounded-lg border border-border">
        <table className="w-full table-fixed">
          <thead className="sticky top-0 z-10 bg-background">
            <tr>
              <th className="w-10 px-3">
                <Checkbox
                  aria-label={t("selectAll")}
                  checked={
                    visible.length > 0 &&
                    visible.every((item) => selected.includes(item.accountId))
                  }
                  onCheckedChange={(checked) =>
                    setSelected(
                      checked ? visible.map((entry) => entry.accountId) : [],
                    )
                  }
                />
              </th>
              <th className={th}>{t("accounts")}</th>
              <th className={`${th} hidden lg:table-cell`}>
                {t("workspaces")}
              </th>
              <th className={`${th} w-32`}>{t("status")}</th>
              <th className={`${th} w-28 text-right`}>{t("actions")}</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {loading
              ? Array.from({ length: 4 }, (_, index) => (
                  <tr key={index}>
                    <td colSpan={5} className="px-3 py-3">
                      <div className="h-5 animate-pulse rounded bg-muted" />
                    </td>
                  </tr>
                ))
              : visible.map((entry) => {
                  return (
                    <tr key={entry.accountId} className="hover:bg-muted/30">
                      <td className="px-3">
                        <Checkbox
                          aria-label={`${t("select")} ${entry.username || entry.email}`}
                          checked={selected.includes(entry.accountId)}
                          onCheckedChange={(checked) =>
                            setSelected((items) =>
                              checked
                                ? [...items, entry.accountId]
                                : items.filter((id) => id !== entry.accountId),
                            )
                          }
                        />
                      </td>
                      <td className={td}>
                        <div className="truncate font-medium">
                          {entry.displayName ||
                            entry.username ||
                            entry.email ||
                            t("accounts")}
                        </div>
                        <div className="truncate text-xs text-muted-foreground">
                          {[entry.username, entry.email]
                            .filter(Boolean)
                            .join(" · ")}
                        </div>
                      </td>
                      <td
                        className={`${td} hidden truncate text-xs text-muted-foreground lg:table-cell`}
                      >
                        {entry.workspaceNames.join(", ") || "—"}
                      </td>
                      <td className={td}>
                        <div className="flex flex-col gap-1 text-xs">
                          {accountDirectoryStatuses(
                            organizationId
                              ? "organization"
                              : workspaceId
                                ? "workspace"
                                : "platform",
                            {
                              accountStatus: entry.status,
                              organizationMembershipStatus: entry.orgStatus,
                              workspaceMembershipStatus: entry.workspaceStatus,
                            },
                          ).map((row) => (
                            <span key={row.label}>
                              {t(row.label)}: {t(row.value ?? "notAvailable")}
                            </span>
                          ))}
                        </div>
                      </td>
                      <td className={`${td} text-right`}>
                        {editable(entry) ||
                        canAssignWorkspace(entry) ||
                        viewer?.platformAdmin ||
                        (organizationId &&
                          canManageOrganization &&
                          entry.orgMemberId &&
                          entry.accountId !== organizationOwnerId) ? (
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <Button
                                variant="ghost"
                                size="icon"
                                className="size-8"
                                aria-label={t("actions")}
                              >
                                <LuEllipsis />
                              </Button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="end">
                              {editable(entry) ? (
                                <DropdownMenuItem
                                  onSelect={() => beginEdit([entry])}
                                >
                                  <LuPencil />
                                  {t("editAccount")}
                                </DropdownMenuItem>
                              ) : null}
                              {viewer?.platformAdmin &&
                              entry.accountId !== viewer.accountId ? (
                                <DropdownMenuItem
                                  onSelect={() => {
                                    setPasswordTarget(entry);
                                    setResetPassword("");
                                    setResetPasswordConfirm("");
                                    setResetPasswordError("");
                                  }}
                                >
                                  <LuKeyRound />
                                  {t("adminResetPassword")}
                                </DropdownMenuItem>
                              ) : null}
                              {canAssignWorkspace(entry) ? (
                                <DropdownMenuItem
                                  onSelect={() => {
                                    setSelected([entry.accountId]);
                                    setAssignWorkspaceIds([]);
                                    setAssigning(true);
                                  }}
                                >
                                  <LuShield />
                                  {t("assignWorkspace")}
                                </DropdownMenuItem>
                              ) : null}
                              {organizationId &&
                              canManageOrganization &&
                              entry.orgMemberId &&
                              entry.accountId !== organizationOwnerId ? (
                                <>
                                  <DropdownMenuSeparator />
                                  <DropdownMenuItem
                                    onSelect={() => {
                                      setSelected([entry.accountId]);
                                      setOrgStatus(
                                        entry.orgStatus === "active"
                                          ? "suspended"
                                          : "active",
                                      );
                                    }}
                                  >
                                    {t(
                                      entry.orgStatus === "active"
                                        ? "orgSuspend"
                                        : "orgRestore",
                                    )}
                                  </DropdownMenuItem>
                                </>
                              ) : null}
                              {!organizationId && viewer?.platformAdmin ? (
                                <>
                                  <DropdownMenuSeparator />
                                  <DropdownMenuItem
                                    onSelect={() => {
                                      setSelected([entry.accountId]);
                                      setStatus(
                                        entry.status === "active"
                                          ? "suspended"
                                          : "active",
                                      );
                                    }}
                                  >
                                    {t(
                                      entry.status === "active"
                                        ? "suspend"
                                        : "activate",
                                    )}
                                  </DropdownMenuItem>
                                </>
                              ) : null}
                            </DropdownMenuContent>
                          </DropdownMenu>
                        ) : (
                          <span className="text-xs text-muted-foreground">
                            —
                          </span>
                        )}
                      </td>
                    </tr>
                  );
                })}
          </tbody>
        </table>
        {!loading && visible.length === 0 ? (
          <div className="p-10 text-center text-sm text-muted-foreground">
            {t(query ? "noResults" : "noAccounts")}
          </div>
        ) : null}
      </div>
      {outcomes.length ? (
        <div className="space-y-2 rounded-md border border-border p-3">
          <div className="max-h-40 overflow-auto">
            {outcomes.map((result, index) => (
              <div
                key={`${result.label}-${index}`}
                className={`flex justify-between gap-3 py-1 text-xs ${result.failed ? "text-destructive" : "text-muted-foreground"}`}
              >
                <span>{result.label}</span>
                <span>{result.result}</span>
              </div>
            ))}
          </div>
          {statusRetry ? (
            <Button
              variant="outline"
              size="sm"
              disabled={pending || loading || Boolean(error)}
              onClick={() => {
                setSelected(statusRetry.accountIds);
                if (statusRetry.scope === "organization") {
                  setOrgStatus(statusRetry.status);
                } else {
                  setStatus(statusRetry.status);
                }
              }}
            >
              <LuRefreshCw />
              {t("retryFailed")}
            </Button>
          ) : null}
        </div>
      ) : null}
      {provisioning ? (
        <AccountProvisioning
          open
          onOpenChange={(open) => {
            if (!open) setProvisioning(undefined);
          }}
          organizationId={organizationId}
          workspaceId={workspaceId}
          initialMode={provisioning}
          onCreated={() => {
            void reload();
            onDataChanged?.();
          }}
        />
      ) : null}
      <Dialog
        open={editing.length > 0}
        onOpenChange={(open) => {
          if (!open && !pending) setEditing([]);
        }}
      >
        <DialogContent
          className={editing.length > 1 ? "max-w-4xl" : "max-w-xl"}
          dismissible={!pending}
        >
          <DialogHeader>
            <DialogTitle>
              {t(editing.length > 1 ? "editMany" : "editAccount")}
            </DialogTitle>
            <DialogDescription>{t("blankKeeps")}</DialogDescription>
          </DialogHeader>
          <AccountError message={error} />
          {editing.some((entry) => !editable(entry)) ? (
            <p className="text-sm text-muted-foreground">
              {t("forbiddenEdit")}
            </p>
          ) : null}
          <div className="max-h-96 space-y-3 overflow-auto">
            {editing.map((entry) => (
              <div
                key={entry.accountId}
                className="space-y-3 rounded-lg border border-border p-3"
              >
                <div className="text-sm font-medium">
                  {entry.username || entry.email}
                </div>
                <div className="grid gap-3 sm:grid-cols-2">
                  <div className="space-y-1.5">
                    <Label htmlFor={`account-${entry.accountId}-display-name`}>
                      {t("displayName")}
                    </Label>
                    <Input
                      id={`account-${entry.accountId}-display-name`}
                      disabled={!editable(entry)}
                      value={editValues[entry.accountId]?.displayName ?? ""}
                      onChange={(event) =>
                        setEditValues((values) => ({
                          ...values,
                          [entry.accountId]: {
                            ...values[entry.accountId],
                            displayName: event.target.value,
                          },
                        }))
                      }
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor={`account-${entry.accountId}-username`}>
                      {t("username")}
                    </Label>
                    <Input
                      id={`account-${entry.accountId}-username`}
                      disabled={!editable(entry)}
                      value={editValues[entry.accountId]?.username ?? ""}
                      onChange={(event) =>
                        setEditValues((values) => ({
                          ...values,
                          [entry.accountId]: {
                            ...values[entry.accountId],
                            username: event.target.value,
                          },
                        }))
                      }
                    />
                  </div>
                  <div className="space-y-1.5 sm:col-span-2">
                    <Label htmlFor={`account-${entry.accountId}-email`}>
                      {t("email")}
                    </Label>
                    <Input
                      id={`account-${entry.accountId}-email`}
                      disabled={!editable(entry)}
                      value={editValues[entry.accountId]?.email ?? ""}
                      onChange={(event) =>
                        setEditValues((values) => ({
                          ...values,
                          [entry.accountId]: {
                            ...values[entry.accountId],
                            email: event.target.value,
                          },
                        }))
                      }
                    />
                  </div>
                  {viewer?.accountId === entry.accountId &&
                  (editValues[entry.accountId]?.username !== entry.username ||
                    editValues[entry.accountId]?.email !== entry.email) ? (
                    <div className="space-y-1.5 sm:col-span-2">
                      <Label
                        htmlFor={`account-${entry.accountId}-current-password`}
                      >
                        {t("currentPassword")}
                      </Label>
                      <Input
                        id={`account-${entry.accountId}-current-password`}
                        type="password"
                        autoComplete="current-password"
                        value={
                          editValues[entry.accountId]?.currentPassword ?? ""
                        }
                        onChange={(event) =>
                          setEditValues((values) => ({
                            ...values,
                            [entry.accountId]: {
                              ...values[entry.accountId],
                              currentPassword: event.target.value,
                            },
                          }))
                        }
                      />
                    </div>
                  ) : null}
                </div>
                {entry.workspaceNames.length ? (
                  <div className="text-xs text-muted-foreground">
                    {t("workspaces")}: {entry.workspaceNames.join(", ")}
                  </div>
                ) : null}
                {editResults[entry.accountId] ? (
                  <p role="status" className="text-xs">
                    {editResults[entry.accountId]}
                  </p>
                ) : null}
              </div>
            ))}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => setEditing([])}
            >
              {t("close")}
            </Button>
            <Button
              disabled={pending || editing.every((entry) => !editable(entry))}
              onClick={() => void saveEdits()}
            >
              {pending ? t("processing") : t("save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={assigning}
        onOpenChange={(open) => {
          if (!pending) setAssigning(open);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("assignWorkspace")}</DialogTitle>
            <DialogDescription>
              {t("assignWorkspacePreview", {
                accounts: selected.length,
                workspaces: assignWorkspaceIds.length,
                assignments: selectedEntries.reduce(
                  (count, entry) =>
                    count +
                    assignWorkspaceIds.filter(
                      (id) => !entry.workspaceIds.includes(id),
                    ).length,
                  0,
                ),
              })}
            </DialogDescription>
          </DialogHeader>
          <AccountError message={error} />
          <WorkspaceTagPicker
            label={t("addWorkspace")}
            options={assignableWorkspaces}
            selected={assignWorkspaceIds}
            onChange={setAssignWorkspaceIds}
            unavailableIds={unavailableWorkspaceIds}
            disabled={pending}
          />
          <RolePicker value={assignRole} onChange={setAssignRole} />
          <p className="text-xs text-muted-foreground">
            {t(`${assignRole}Help`)} {t("roleHint")}
          </p>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setAssigning(false)}
              disabled={pending}
            >
              {t("cancel")}
            </Button>
            <Button
              disabled={
                pending ||
                !assignWorkspaceIds.length ||
                !selected.length ||
                unavailableWorkspaceIds.length === assignWorkspaceIds.length
              }
              onClick={() => void assign()}
            >
              {pending
                ? t("processing")
                : t("assignWorkspaceCount", {
                    count: assignWorkspaceIds.length,
                  })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={Boolean(passwordTarget)}
        onOpenChange={(open) => {
          if (!open && !pending) {
            setPasswordTarget(undefined);
            setResetPassword("");
            setResetPasswordConfirm("");
            setResetPasswordError("");
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("adminResetPassword")}</DialogTitle>
            <DialogDescription>
              {t("adminResetPasswordHint", {
                account:
                  passwordTarget?.username || passwordTarget?.email || "",
              })}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3">
            <div className="space-y-1.5">
              <Label htmlFor="admin-account-new-password">
                {t("newPassword")}
              </Label>
              <Input
                id="admin-account-new-password"
                type="password"
                autoComplete="new-password"
                value={resetPassword}
                onChange={(event) => setResetPassword(event.target.value)}
                disabled={pending}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="admin-account-confirm-password">
                {t("confirmPassword")}
              </Label>
              <Input
                id="admin-account-confirm-password"
                type="password"
                autoComplete="new-password"
                value={resetPasswordConfirm}
                onChange={(event) =>
                  setResetPasswordConfirm(event.target.value)
                }
                disabled={pending}
              />
            </div>
            <p className="text-xs text-muted-foreground">
              {t("adminPasswordResetImpact")}
            </p>
            {resetPasswordError ? (
              <AccountError message={resetPasswordError} />
            ) : null}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => setPasswordTarget(undefined)}
            >
              {t("cancel")}
            </Button>
            <Button
              disabled={pending || !resetPassword || !resetPasswordConfirm}
              onClick={() => void resetAccountPassword()}
            >
              {pending ? t("processing") : t("adminResetPassword")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={Boolean(status)}
        onOpenChange={(open) => {
          if (!open && !pending) setStatus(undefined);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("confirmChange")}</DialogTitle>
            <DialogDescription>
              {t("selected", { count: selected.length })} ·{" "}
              {status ? t(status) : ""}
            </DialogDescription>
          </DialogHeader>
          <ul className="max-h-48 overflow-auto text-sm">
            {selectedEntries.map((entry) => (
              <li className="py-1" key={entry.accountId}>
                {entry.username || entry.email}:{" "}
                {t(entry.status ?? "notAvailable")} → {status ? t(status) : ""}
              </li>
            ))}
          </ul>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => setStatus(undefined)}
            >
              {t("cancel")}
            </Button>
            <Button disabled={pending} onClick={() => void changeStatus()}>
              {pending ? t("processing") : t("confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={Boolean(orgStatus)}
        onOpenChange={(open) => {
          if (!open && !pending) setOrgStatus(undefined);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t(orgStatus === "suspended" ? "orgSuspend" : "orgRestore")}
            </DialogTitle>
            <DialogDescription>
              {t("organizationMemberStatusHint")}
            </DialogDescription>
          </DialogHeader>
          <ul className="max-h-48 overflow-auto text-sm">
            {selectedEntries.map((entry) => (
              <li className="py-1" key={entry.accountId}>
                {entry.username || entry.email}:{" "}
                {t(entry.orgStatus ?? "active")} → {t(orgStatus ?? "active")}
              </li>
            ))}
          </ul>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => setOrgStatus(undefined)}
            >
              {t("cancel")}
            </Button>
            <Button
              disabled={pending}
              onClick={() => void changeOrganizationStatus()}
            >
              {pending ? t("processing") : t("confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function LuUsersIcon() {
  return <LuUserPlus />;
}
