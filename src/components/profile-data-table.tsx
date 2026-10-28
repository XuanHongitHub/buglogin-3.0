"use client";

import {
  flexRender,
  type RowSelectionState,
  type SortingState,
  type ColumnVisibilityState as VisibilityState,
} from "@tanstack/react-table";
import {
  type LegacyColumnDef as ColumnDef,
  getCoreRowModel,
  getSortedRowModel,
  useLegacyTable as useReactTable,
} from "@tanstack/react-table/legacy";
import { useVirtualizer } from "@tanstack/react-virtual";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import type { Dispatch, SetStateAction } from "react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { FiWifi } from "react-icons/fi";
import {
  LuCheck,
  LuChevronDown,
  LuChevronUp,
  LuCookie,
  LuCopy,
  LuEllipsis,
  LuFolder,
  LuFolderPlus,
  LuGlobe,
  LuInfo,
  LuLock,
  LuPlay,
  LuPlus,
  LuPuzzle,
  LuRefreshCw,
  LuShieldCheck,
  LuSparkles,
  LuSquare,
  LuTrash2,
  LuTriangleAlert,
  LuUserPlus,
  LuUserSearch,
  LuUsers,
} from "react-icons/lu";
import { toast } from "sonner";
import { DeleteConfirmationDialog } from "@/components/delete-confirmation-dialog";
import {
  ProfileGroupDragHandle,
  useProfileGroupDrag,
} from "@/components/profile-group-drag";
import { ProfileHandoffStatus } from "@/components/profile-handoff-status";
import {
  ProfileBypassRulesDialog,
  ProfileDnsBlocklistDialog,
  ProfileInfoDialog,
  ProfileLaunchHookDialog,
} from "@/components/profile-info-dialog";
import {
  DATA_TABLE_CLASSES,
  TableColumnResizer,
  TableRowCheckbox,
  TableSelectHeader,
  TableViewControls,
} from "@/components/table-controls";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { ProBadge } from "@/components/ui/pro-badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useBrowserState } from "@/hooks/use-browser-state";
import { useCloudAuth } from "@/hooks/use-cloud-auth";
import { useScrollFade } from "@/hooks/use-scroll-fade";
import { useTablePreferences } from "@/hooks/use-table-preferences";
import { useTableSorting } from "@/hooks/use-table-sorting";
import { useTeamLocks } from "@/hooks/use-team-locks";
import {
  getBrowserDisplayName,
  getOSDisplayName,
  getOSIcon,
  getProfileIcon,
  isCrossOsProfile,
} from "@/lib/browser-utils";
import { DNS_BLOCKLIST_LEVELS } from "@/lib/dns-blocklist-levels";
import { extractRootError, formatError } from "@/lib/error-utils";
import { formatRelativeTime } from "@/lib/flag-utils";
import { showErrorToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import type {
  BrowserProfile,
  ExtensionGroup,
  LocationItem,
  ProfileLockInfo,
  ProxyCheckResult,
  StoredProxy,
  SyncSessionInfo,
  TrafficSnapshot,
} from "@/types";
import { BandwidthMiniChart } from "./bandwidth-mini-chart";
import {
  DataTableActionBar,
  DataTableActionBarAction,
  DataTableActionBarSelection,
} from "./data-table-action-bar";
import { Logo } from "./icons/logo";
import MultipleSelector, { type Option } from "./multiple-selector";
import { ProxyCheckButton } from "./proxy-check-button";
import { TrafficDetailsDialog } from "./traffic-details-dialog";
import { Input } from "./ui/input";
import { RippleButton } from "./ui/ripple";
import { Skeleton } from "./ui/skeleton";

/**
 * The launch control's tooltip. Launch progress stays in the button/activity
 * surface; keeping a live tooltip here causes it to remain visible while the
 * clicked button retains keyboard focus, even after the pointer leaves.
 */
function LaunchControlTooltip({
  launching,
  content,
  children,
}: {
  launching: boolean;
  content: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      {!launching && content ? (
        <TooltipContent>{content}</TooltipContent>
      ) : null}
    </Tooltip>
  );
}

/**
 * An empty cell renders a dash (—) normally, but reveals an action hint like
 * "Add note" or "Add tags" when the row/cell is hovered. The accessible
 * label stays for screen readers, and an editable cell names its action
 * while the pointer is over it.
 */
function EmptyCellValue({
  label,
  hint,
}: {
  label: string;
  hint?: string | null;
}) {
  return (
    <span className="flex min-w-0 flex-1 truncate text-sm text-muted-foreground">
      <span
        aria-hidden="true"
        className={cn(
          "inline-block",
          hint && "group-hover/cell:hidden group-focus-within/cell:hidden",
        )}
      >
        —
      </span>
      {hint ? (
        <span
          aria-hidden="true"
          className="hidden truncate text-muted-foreground/80 group-hover/cell:inline group-focus-within/cell:inline"
        >
          {hint}
        </span>
      ) : null}
      <span className="sr-only">{label}</span>
    </span>
  );
}

// Stable table meta type to pass volatile state/handlers into TanStack Table without
// causing column definitions to be recreated on every render.
interface TableMeta {
  workspaceId: string | null;
  t: (key: string, options?: Record<string, unknown>) => string;
  selectedProfiles: string[];
  selectableCount: number;
  showCheckboxes: boolean;
  selectedDragProfiles: BrowserProfile[];
  canDragProfiles: (profiles: BrowserProfile[]) => boolean;
  getRemoteHandoff?: (profileId: string) => string | null;
  onOpenProfileSyncDialog?: (profile: BrowserProfile) => void;
  isClient: boolean;
  runningProfiles: Set<string>;
  runtimeRunningProfileIds: Set<string>;
  launchingProfiles: Set<string>;
  stoppingProfiles: Set<string>;
  isUpdating: (browser: string) => boolean;
  browserState: ReturnType<typeof useBrowserState>;

  // Tags editor state
  tagsOverrides: Record<string, string[]>;
  allTags: string[];
  openTagsEditorFor: string | null;
  setAllTags: React.Dispatch<React.SetStateAction<string[]>>;
  setOpenTagsEditorFor: React.Dispatch<React.SetStateAction<string | null>>;
  setTagsOverrides: React.Dispatch<
    React.SetStateAction<Record<string, string[]>>
  >;

  // Note editor state
  noteOverrides: Record<string, string | null>;
  openNoteEditorFor: string | null;
  setOpenNoteEditorFor: React.Dispatch<React.SetStateAction<string | null>>;
  setNoteOverrides: React.Dispatch<
    React.SetStateAction<Record<string, string | null>>
  >;

  // Proxy selector state
  openProxySelectorFor: string | null;
  setOpenProxySelectorFor: React.Dispatch<React.SetStateAction<string | null>>;
  proxyOverrides: Record<string, string | null>;
  storedProxies: StoredProxy[];
  handleProxySelection: (
    profileId: string,
    proxyId: string | null,
  ) => Promise<void>;
  checkingProfileId: string | null;
  proxyCheckResults: Record<string, ProxyCheckResult>;

  // Extension groups (for Ext column lookup)
  extensionGroups: ExtensionGroup[];

  // Click handlers for inline Ext / DNS cell editing
  onAssignExtensionGroup?: (profileIds: string[]) => void;
  setDnsBlocklistProfile: React.Dispatch<
    React.SetStateAction<BrowserProfile | null>
  >;

  // Selection helpers
  isProfileSelected: (id: string) => boolean;
  handleToggleAll: (checked: boolean) => void;
  handleCheckboxChange: (id: string, checked: boolean) => void;
  handleIconClick: (id: string) => void;

  // Rename helpers
  handleRename: () => void | Promise<void>;
  setProfileToRename: React.Dispatch<
    React.SetStateAction<BrowserProfile | null>
  >;
  setNewProfileName: React.Dispatch<React.SetStateAction<string>>;
  setRenameError: React.Dispatch<React.SetStateAction<string | null>>;
  profileToRename: BrowserProfile | null;
  newProfileName: string;
  isRenamingSaving: boolean;
  renameError: string | null;

  // Launch/stop helpers
  setLaunchingProfiles: React.Dispatch<React.SetStateAction<Set<string>>>;
  setStoppingProfiles: React.Dispatch<React.SetStateAction<Set<string>>>;
  onKillProfile: (profile: BrowserProfile) => void | Promise<void>;
  onLaunchProfile: (profile: BrowserProfile) => void | Promise<void>;

  // Overflow actions
  onAssignProfilesToGroup?: (profileIds: string[]) => void;
  onConfigureWayfern?: (profile: BrowserProfile) => void;
  onCloneProfile?: (profile: BrowserProfile) => void;
  onShareProfile?: (profile: BrowserProfile) => void;
  onCopyCookiesToProfile?: (profile: BrowserProfile) => void;
  onOpenCookieManagement?: (profile: BrowserProfile) => void;

  // Traffic snapshots (lightweight real-time data)
  trafficSnapshots: Record<string, TrafficSnapshot>;
  onOpenTrafficDialog?: (profileId: string) => void;

  // Sync
  syncStatuses: Record<
    string,
    {
      status: string;
      error?: string;
      generation?: number;
      can_open?: boolean;
      can_open_error?: string;
      checkedAt?: number;
      source?: "api";
    }
  >;
  refreshProfileSyncStatus?: (profileId: string, force?: boolean) => void;
  crossOsUnlocked?: boolean;
  syncUnlocked?: boolean;

  // Country proxy creation (inline in proxy dropdown)
  countries: LocationItem[];
  canCreateLocationProxy: boolean;
  loadCountries: () => Promise<void>;
  handleCreateCountryProxy: (
    profileId: string,
    country: LocationItem,
  ) => Promise<void>;

  // Team locks
  isProfileLockedByAnother: (profileId: string) => boolean;
  getProfileLockEmail: (profileId: string) => string | undefined;
  getProfileLockInfo: (profileId: string) => ProfileLockInfo | undefined;
  refreshProfileLockStatus: () => Promise<void>;

  // Synchronizer
  getProfileSyncInfo: (profileId: string) =>
    | {
        session: SyncSessionInfo;
        isLeader: boolean;
        failedAtUrl: string | null;
      }
    | undefined;
  onLaunchWithSync: (profile: BrowserProfile) => void;
}

interface SyncStatusDot {
  color: string;
  tooltip: string;
  animate: boolean;
  encrypted: boolean;
}

function getProfileSyncStatusDot(
  profile: BrowserProfile,
  liveStatus:
    | "syncing"
    | "waiting"
    | "synced"
    | "error"
    | "disabled"
    | undefined,
  t: (key: string, options?: Record<string, unknown>) => string,
  errorMessage?: string,
): SyncStatusDot | null {
  const encrypted =
    profile.sync_mode === "managed" || profile.sync_mode === "Encrypted";
  const managed =
    profile.sync_mode === "managed" ||
    profile.sync_mode === "Encrypted" ||
    profile.sync_mode === "Regular";

  if (!profile.last_sync && !liveStatus) {
    if (profile.sync_mode === "Disabled" || profile.sync_mode === "disabled") {
      return null;
    }
    if (!managed) {
      return {
        color: "bg-muted-foreground/40",
        tooltip: t("profileTable.syncTooltipNotSynced", {
          defaultValue: "Profile này chỉ lưu trên máy, chưa tải lên Cloud.",
        }),
        animate: false,
        encrypted,
      };
    }
  }

  const status =
    liveStatus ??
    (profile.last_sync
      ? "synced"
      : profile.sync_mode === "managed" ||
          profile.sync_mode === "Regular" ||
          profile.sync_mode === "Encrypted"
        ? "waiting"
        : "disabled");

  switch (status) {
    case "syncing":
      return {
        color: "bg-warning",
        tooltip: t("profileTable.syncTooltipSyncing"),
        animate: true,
        encrypted,
      };
    case "waiting":
      return {
        color: "bg-muted-foreground/40",
        tooltip: t("profileTable.syncTooltipCloseToSync"),
        animate: false,
        encrypted,
      };
    case "synced":
      return {
        color: "bg-success",
        tooltip: profile.last_sync
          ? t("profileTable.syncTooltipSyncedAt", {
              time: new Date(profile.last_sync * 1000).toLocaleString(),
            })
          : t("profileTable.syncTooltipSynced"),
        animate: false,
        encrypted,
      };
    case "error":
      return {
        color: "bg-destructive",
        tooltip: errorMessage
          ? t("profileTable.syncTooltipErrorWith", { error: errorMessage })
          : t("profileTable.syncTooltipError"),
        animate: false,
        encrypted,
      };
    case "disabled":
      if (profile.last_sync) {
        return {
          color: "bg-muted-foreground",
          tooltip: t("profileTable.syncTooltipDisabledWithLast", {
            time: formatRelativeTime(profile.last_sync),
          }),
          animate: false,
          encrypted: false,
        };
      }
      return null;
    default:
      return null;
  }
}

// Inline extension-group dropdown for the Ext column. Matches the
// proxy column's Popover-style picker — no nested dialog.
function ExtCell({
  profile,
  meta,
}: {
  profile: BrowserProfile;
  meta: TableMeta;
}) {
  const [open, setOpen] = React.useState(false);
  const [isSaving, setIsSaving] = React.useState(false);
  const groupId = profile.extension_group_id ?? null;
  const group = groupId
    ? meta.extensionGroups.find((g) => g.id === groupId)
    : undefined;
  const label =
    group?.name ?? meta.t("profiles.table.none", { defaultValue: "None" });

  const onPick = async (nextId: string | null) => {
    setIsSaving(true);
    try {
      await invoke("assign_extension_group_to_profile", {
        workspaceId: meta.workspaceId,
        profileId: profile.id,
        extensionGroupId: nextId,
      });
    } catch (err) {
      console.error("Failed to assign extension group:", err);
    } finally {
      setIsSaving(false);
      setOpen(false);
    }
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={isSaving}
          className="group/cell flex h-6 w-full items-center rounded-md px-2 text-left text-sm transition-colors duration-100 hover:bg-foreground/6 disabled:opacity-50"
        >
          {group ? (
            <span className="flex-1 truncate" title={label}>
              {group.name}
            </span>
          ) : (
            <EmptyCellValue label={label} />
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-56 p-0" align="start">
        <Command>
          <CommandInput placeholder={meta.t("profiles.table.extSearch")} />
          <CommandList>
            <CommandEmpty>{meta.t("profiles.table.extEmpty")}</CommandEmpty>
            <CommandGroup>
              <CommandItem
                value="__default__"
                onSelect={() => {
                  void onPick(null);
                }}
              >
                {groupId === null && <LuCheck className="mr-2 size-3.5" />}
                <span className={groupId === null ? "" : "ml-5"}>
                  {meta.t("profiles.table.none", { defaultValue: "None" })}
                </span>
              </CommandItem>
              {meta.extensionGroups.map((g) => (
                <CommandItem
                  key={g.id}
                  value={g.name}
                  onSelect={() => {
                    void onPick(g.id);
                  }}
                >
                  {groupId === g.id && <LuCheck className="mr-2 size-3.5" />}
                  <span className={groupId === g.id ? "" : "ml-5"}>
                    {g.name}
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

// Inline DNS blocklist dropdown — same Popover/Command pattern as Ext.
function DnsCell({
  profile,
  meta,
}: {
  profile: BrowserProfile;
  meta: TableMeta;
}) {
  const [open, setOpen] = React.useState(false);
  const [isSaving, setIsSaving] = React.useState(false);
  const level = profile.dns_blocklist ?? null;
  const LEVELS = DNS_BLOCKLIST_LEVELS;
  const currentLabel =
    level === null
      ? null
      : (LEVELS.find((l) => l.value === level)?.labelKey ?? null);

  const onPick = async (nextLevel: string | null) => {
    setIsSaving(true);
    try {
      await invoke("update_profile_dns_blocklist", {
        profileId: profile.id,
        dnsBlocklist: nextLevel,
      });
    } catch (err) {
      console.error("Failed to update DNS blocklist:", err);
    } finally {
      setIsSaving(false);
      setOpen(false);
    }
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          data-onborda="dns-blocklist"
          disabled={isSaving}
          className="group/cell flex h-6 w-full items-center rounded-md px-2 text-left text-sm transition-colors duration-100 hover:bg-foreground/6 disabled:opacity-50"
          title={
            level
              ? meta.t("profiles.table.dnsLevel", { level })
              : meta.t("dnsBlocklist.none")
          }
        >
          {currentLabel ? (
            <span className="flex-1 truncate">{meta.t(currentLabel)}</span>
          ) : (
            <EmptyCellValue
              label={meta.t("dnsBlocklist.none", { defaultValue: "None" })}
            />
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-48 p-0" align="start">
        <Command>
          <CommandList>
            <CommandGroup>
              <CommandItem
                value="__none__"
                onSelect={() => {
                  void onPick(null);
                }}
              >
                {level === null && <LuCheck className="mr-2 size-3.5" />}
                <span className={level === null ? "" : "ml-5"}>
                  {meta.t("dnsBlocklist.none")}
                </span>
              </CommandItem>
              {LEVELS.map((l) => (
                <CommandItem
                  key={l.value}
                  value={l.value}
                  onSelect={() => {
                    void onPick(l.value);
                  }}
                >
                  {level === l.value && <LuCheck className="mr-2 size-3.5" />}
                  <span className={level === l.value ? "" : "ml-5"}>
                    {meta.t(l.labelKey)}
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

const TagsCell = React.memo<{
  profile: BrowserProfile;
  isDisabled: boolean;
  tagsOverrides: Record<string, string[]>;
  allTags: string[];
  setAllTags: React.Dispatch<React.SetStateAction<string[]>>;
  openTagsEditorFor: string | null;
  setOpenTagsEditorFor: React.Dispatch<React.SetStateAction<string | null>>;
  setTagsOverrides: React.Dispatch<
    React.SetStateAction<Record<string, string[]>>
  >;
}>(
  ({
    profile,
    isDisabled,
    tagsOverrides,
    allTags,
    setAllTags,
    openTagsEditorFor,
    setOpenTagsEditorFor,
    setTagsOverrides,
  }) => {
    const { t: translate } = useTranslation();
    const effectiveTags: string[] = Object.hasOwn(tagsOverrides, profile.id)
      ? tagsOverrides[profile.id]
      : (profile.tags ?? []);

    const valueOptions: Option[] = React.useMemo(
      () => effectiveTags.map((t) => ({ value: t, label: t })),
      [effectiveTags],
    );
    const allOptions: Option[] = React.useMemo(
      () => allTags.map((t) => ({ value: t, label: t })),
      [allTags],
    );

    const onTagsChange = React.useCallback(
      async (newTagsRaw: string[]) => {
        // Dedupe tags
        const seen = new Set<string>();
        const newTags: string[] = [];
        for (const t of newTagsRaw) {
          if (!seen.has(t)) {
            seen.add(t);
            newTags.push(t);
          }
        }
        setTagsOverrides((prev) => ({ ...prev, [profile.id]: newTags }));
        try {
          await invoke<BrowserProfile>("update_profile_tags", {
            profileId: profile.id,
            tags: newTags,
          });
          setAllTags((prev) => {
            const next = new Set(prev);
            for (const t of newTags) next.add(t);
            return Array.from(next).sort();
          });
        } catch (error) {
          console.error("Failed to update tags:", error);
          setTagsOverrides((prev) => {
            const next = { ...prev };
            delete next[profile.id];
            return next;
          });
          toast.error(error instanceof Error ? error.message : String(error));
        }
      },
      [profile.id, setTagsOverrides, setAllTags],
    );

    const handleChange = React.useCallback(
      async (opts: Option[]) => {
        const newTagsRaw = opts.map((o) => o.value);
        await onTagsChange(newTagsRaw);
      },
      [onTagsChange],
    );

    const containerRef = React.useRef<HTMLDivElement | null>(null);
    const editorRef = React.useRef<HTMLDivElement | null>(null);
    const [visibleCount, setVisibleCount] = React.useState<number>(
      effectiveTags.length,
    );
    const [isFocused, setIsFocused] = React.useState(false);

    React.useLayoutEffect(() => {
      // Only measure when not editing this profile's tags
      if (openTagsEditorFor === profile.id) return;
      const container = containerRef.current;
      if (!container) return;

      let timeoutId: number | undefined;
      const compute = () => {
        if (timeoutId) clearTimeout(timeoutId);
        timeoutId = window.setTimeout(() => {
          const available = container.clientWidth;
          if (available <= 0) return;
          const canvas = document.createElement("canvas");
          const ctx = canvas.getContext("2d");
          if (!ctx) return;
          const style = window.getComputedStyle(container);
          const font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
          ctx.font = font;
          const padding = 16;
          const gap = 4;
          let used = 0;
          let count = 0;
          for (let i = 0; i < effectiveTags.length; i++) {
            const text = effectiveTags[i];
            const width = Math.ceil(ctx.measureText(text).width) + padding;
            const remaining = effectiveTags.length - (i + 1);
            let extra = 0;
            if (remaining > 0) {
              const plusText = `+${remaining}`;
              extra = Math.ceil(ctx.measureText(plusText).width) + padding;
            }
            const nextUsed =
              used +
              (used > 0 ? gap : 0) +
              width +
              (remaining > 0 ? gap + extra : 0);
            if (nextUsed <= available) {
              used += (used > 0 ? gap : 0) + width;
              count = i + 1;
            } else {
              break;
            }
          }
          setVisibleCount(count);
        }, 16); // Debounce with RAF timing
      };
      compute();
      const ro = new ResizeObserver(compute);
      ro.observe(container);
      return () => {
        ro.disconnect();
        if (timeoutId) clearTimeout(timeoutId);
      };
    }, [effectiveTags, openTagsEditorFor, profile.id]);

    React.useEffect(() => {
      if (openTagsEditorFor !== profile.id) return;
      const handleClick = (e: MouseEvent) => {
        const target = e.target as Node | null;
        if (
          editorRef.current &&
          target &&
          !editorRef.current.contains(target)
        ) {
          setOpenTagsEditorFor(null);
        }
      };
      document.addEventListener("mousedown", handleClick);
      return () => {
        document.removeEventListener("mousedown", handleClick);
      };
    }, [openTagsEditorFor, profile.id, setOpenTagsEditorFor]);

    React.useEffect(() => {
      if (openTagsEditorFor === profile.id && editorRef.current) {
        // Focus the inner input of MultipleSelector on open
        const inputEl = editorRef.current.querySelector("input");
        if (inputEl) {
          inputEl.focus();
        }
      }
    }, [openTagsEditorFor, profile.id]);

    if (openTagsEditorFor !== profile.id) {
      const hiddenCount = Math.max(0, effectiveTags.length - visibleCount);
      const ButtonContent = (
        <button
          type="button"
          ref={containerRef as unknown as React.RefObject<HTMLButtonElement>}
          className={cn(
            "group/cell flex h-6 w-full cursor-pointer items-center gap-1 overflow-hidden rounded-md border-none bg-transparent px-2 py-1 transition-colors duration-100",
            isDisabled
              ? "cursor-not-allowed opacity-60"
              : "cursor-pointer hover:bg-foreground/6",
          )}
          onClick={() => {
            if (!isDisabled) setOpenTagsEditorFor(profile.id);
          }}
        >
          {effectiveTags.slice(0, visibleCount).map((t) => (
            <Badge key={t} variant="soft" className="px-2 py-0 text-xs">
              {t}
            </Badge>
          ))}
          {effectiveTags.length === 0 && (
            <EmptyCellValue
              hint={
                isDisabled ? null : translate("profileTable.addTagsPlaceholder")
              }
              label={translate("profileTable.noTags")}
            />
          )}
          {hiddenCount > 0 && (
            <Badge
              variant="soft"
              className="px-1.5 py-0 text-xs text-muted-foreground"
            >
              +{hiddenCount}
            </Badge>
          )}
        </button>
      );

      return (
        <div className="h-6 w-full cursor-pointer">
          <Tooltip>
            <TooltipTrigger asChild>{ButtonContent}</TooltipTrigger>
            {hiddenCount > 0 && (
              <TooltipContent className="max-w-[320px]">
                <div className="flex flex-wrap gap-1">
                  {effectiveTags.map((t) => (
                    <Badge
                      key={t}
                      variant="secondary"
                      className="px-2 py-0 text-xs"
                    >
                      {t}
                    </Badge>
                  ))}
                </div>
              </TooltipContent>
            )}
          </Tooltip>
        </div>
      );
    }

    return (
      <div
        className={cn(
          "relative h-6 w-full",
          isDisabled && "pointer-events-none opacity-60",
        )}
      >
        <div
          ref={editorRef}
          className="absolute top-0 left-0 z-50 min-h-6 w-40 rounded-md bg-popover shadow-md"
        >
          <MultipleSelector
            value={valueOptions}
            options={allOptions}
            onChange={(opts) => void handleChange(opts)}
            creatable
            selectFirstItem={false}
            placeholder={
              effectiveTags.length === 0
                ? translate("profileTable.addTagsPlaceholder")
                : ""
            }
            className={cn(
              "border-0! bg-transparent focus-within:ring-0!",
              "[&_div:first-child]:border-0! [&_div:first-child]:ring-0! [&_div:first-child]:focus-within:ring-0!",
              "[&_div:first-child]:min-h-6! [&_div:first-child]:px-2! [&_div:first-child]:py-1!",
              "[&_div:first-child>div]:h-6! [&_div:first-child>div]:items-center",
              "[&_input]:mt-0! [&_input]:ml-0! [&_input]:px-0!",
              !isFocused && "[&_div:first-child>div]:justify-center",
            )}
            badgeClassName="shrink-0"
            inputProps={{
              className: "!py-0 text-sm caret-current !ml-0 !mt-0 !px-0",
              onKeyDown: (e) => {
                if (e.key === "Escape") setOpenTagsEditorFor(null);
              },
              onFocus: () => {
                setIsFocused(true);
              },
              onBlur: () => {
                setIsFocused(false);
              },
            }}
          />
        </div>
      </div>
    );
  },
);

TagsCell.displayName = "TagsCell";

const NonHoverableTooltip = React.memo<{
  children: React.ReactNode;
  content: React.ReactNode;
  sideOffset?: number;
  alignOffset?: number;
  horizontalOffset?: number;
}>(
  ({
    children,
    content,
    sideOffset = 4,
    alignOffset = 0,
    horizontalOffset = 0,
  }) => {
    const [isOpen, setIsOpen] = React.useState(false);

    return (
      <Tooltip open={isOpen} onOpenChange={setIsOpen}>
        <TooltipTrigger
          asChild
          onMouseEnter={() => {
            setIsOpen(true);
          }}
          onMouseLeave={() => {
            setIsOpen(false);
          }}
        >
          {children}
        </TooltipTrigger>
        <TooltipContent
          sideOffset={sideOffset}
          alignOffset={alignOffset}
          arrowOffset={horizontalOffset}
          onPointerEnter={(e) => {
            e.preventDefault();
          }}
          onPointerLeave={() => {
            setIsOpen(false);
          }}
          className="pointer-events-none"
          style={
            horizontalOffset !== 0
              ? { transform: `translateX(${horizontalOffset}px)` }
              : undefined
          }
        >
          {content}
        </TooltipContent>
      </Tooltip>
    );
  },
);

NonHoverableTooltip.displayName = "NonHoverableTooltip";

// CSS-truncated text whose tooltip only appears when the text actually
// overflows its column (measured on hover, so it tracks live resizes).
const OverflowTooltipText = React.memo<{
  text: string;
  className?: string;
}>(({ text, className }) => {
  const textRef = React.useRef<HTMLSpanElement | null>(null);
  const [isOverflowing, setIsOverflowing] = React.useState(false);

  return (
    <Tooltip
      onOpenChange={(open) => {
        if (!open) return;
        const el = textRef.current;
        if (el) setIsOverflowing(el.scrollWidth > el.clientWidth);
      }}
    >
      <TooltipTrigger asChild>
        <span
          ref={textRef}
          className={cn("block max-w-full min-w-0 truncate", className)}
        >
          {text}
        </span>
      </TooltipTrigger>
      {isOverflowing && <TooltipContent>{text}</TooltipContent>}
    </Tooltip>
  );
});

OverflowTooltipText.displayName = "OverflowTooltipText";

export function formatProxyClipboard(proxy: StoredProxy): string {
  const { host, port, username, password } = proxy.proxy_settings;
  if (username && password) {
    return `${host}:${port}:${username}:${password}`;
  }
  return `${host}:${port}`;
}

export function parseRawProxy(input: string): {
  host: string;
  port: number;
  username?: string;
  password?: string;
  proxy_type: string;
} | null {
  const trimmed = input.trim();
  if (!trimmed) return null;

  let type = "http";
  let rest = trimmed;
  if (trimmed.includes("://")) {
    const parts = trimmed.split("://");
    type = parts[0].toLowerCase();
    rest = parts[1];
  }

  if (rest.includes("@")) {
    const [cred, hostPort] = rest.split("@");
    const [user, pass] = cred.split(":");
    const [h, p] = hostPort.split(":");
    const portNum = Number.parseInt(p, 10);
    if (h && portNum > 0 && portNum <= 65535) {
      return {
        host: h.trim(),
        port: portNum,
        username: user?.trim(),
        password: pass?.trim(),
        proxy_type: type,
      };
    }
  }

  const parts = rest.split(":");
  if (parts.length >= 2) {
    const host = parts[0].trim();
    const port = Number.parseInt(parts[1], 10);
    if (host && Number.isInteger(port) && port > 0 && port <= 65535) {
      const username = parts[2]?.trim();
      const password = parts.slice(3).join(":").trim();
      return {
        host,
        port,
        username: username || undefined,
        password: password || undefined,
        proxy_type: type,
      };
    }
  }
  return null;
}

function SmartProxyPopoverContent({
  profile,
  effectiveProxy,
  effectiveProxyId,
  storedProxies,
  onSelectProxy,
  onClose,
}: {
  profile: BrowserProfile;
  effectiveProxy: StoredProxy | null;
  effectiveProxyId: string | null;
  storedProxies: StoredProxy[];
  onSelectProxy: (profileId: string, proxyId: string | null) => Promise<void>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const _profileDrag = useProfileGroupDrag();
  const [proxyInput, setProxyInput] = React.useState("");
  const [isApplying, setIsApplying] = React.useState(false);
  const [searchFilter, setSearchFilter] = React.useState("");
  const [activeTab, setActiveTab] = React.useState<"paste" | "saved">("paste");

  const handleApplyRawProxy = async () => {
    const parsed = parseRawProxy(proxyInput);
    if (!parsed) {
      toast.error(
        t("proxies.form.invalidFormat", {
          defaultValue:
            "Định dạng không hợp lệ (hỗ trợ host:port:user:pass hoặc URL)",
        }),
      );
      return;
    }

    setIsApplying(true);
    try {
      const proxyId = `proxy_${crypto.randomUUID()}`;
      await invoke("create_stored_proxy", {
        name: `${parsed.host}:${parsed.port}`,
        proxySettings: {
          proxy_type: parsed.proxy_type,
          host: parsed.host,
          port: parsed.port,
          username: parsed.username,
          password: parsed.password,
        },
        idempotencyKey: crypto.randomUUID(),
        proxyId,
        createdAt: new Date().toISOString(),
      });
      await onSelectProxy(profile.id, proxyId);
      await emit("stored-proxies-changed");
      toast.success(
        t("proxies.assigned", {
          defaultValue: `Đã gán proxy ${parsed.host}:${parsed.port}`,
        }),
      );
      onClose();
    } catch (e) {
      console.error("Failed to create and assign proxy:", e);
      toast.error(
        t("proxies.form.saveFailed", { defaultValue: "Lỗi gán proxy" }),
      );
    } finally {
      setIsApplying(false);
    }
  };

  const handlePasteClipboard = async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text) setProxyInput(text.trim());
    } catch {
      toast.error("Không thể đọc từ clipboard");
    }
  };

  const filteredStored = storedProxies.filter(
    (p) =>
      !p.is_cloud_managed &&
      !p.is_cloud_derived &&
      (p.name.toLowerCase().includes(searchFilter.toLowerCase()) ||
        p.proxy_settings.host.includes(searchFilter)),
  );

  return (
    <div className="w-[320px] p-3 space-y-3 text-xs">
      <div className="flex items-center justify-between border-b border-border/50 pb-2">
        <div className="flex items-center gap-1.5 font-medium">
          <LuGlobe className="size-3.5 text-primary" />
          <span>{t("proxies.title", { defaultValue: "Cấu hình Proxy" })}</span>
          <span className="text-muted-foreground font-normal truncate max-w-[120px]">
            · {profile.name}
          </span>
        </div>
      </div>

      {effectiveProxy && (
        <div className="rounded-md border border-emerald-500/25 bg-emerald-500/10 p-2 space-y-1.5">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-1.5 font-mono text-[11px] font-semibold text-emerald-300 truncate">
              <span className="size-1.5 rounded-full bg-emerald-400 animate-pulse" />
              <span>
                {effectiveProxy.proxy_settings.host}:
                {effectiveProxy.proxy_settings.port}
              </span>
              <span className="text-[10px] text-muted-foreground uppercase">
                ({effectiveProxy.proxy_settings.proxy_type})
              </span>
            </div>
          </div>
          <div className="flex items-center gap-1">
            <Button
              variant="outline"
              size="sm"
              className="h-6 px-2 text-[10px] gap-1"
              onClick={() => {
                const str = formatProxyClipboard(effectiveProxy);
                navigator.clipboard.writeText(str);
                toast.success(`Đã sao chép: ${str}`);
              }}
            >
              <LuCopy className="size-2.5" />
              <span>{t("common.buttons.copy", { defaultValue: "Copy" })}</span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-6 px-2 text-[10px] gap-1"
              onClick={async () => {
                toast.info("Đang kiểm tra kết nối...");
                try {
                  const res = await invoke<ProxyCheckResult>(
                    "check_proxy_validity",
                    {
                      proxySettings: effectiveProxy.proxy_settings,
                    },
                  );
                  if (res.is_valid) {
                    toast.success(`Proxy hoạt động tốt (${res.ip})`);
                  } else {
                    toast.error("Proxy không thể kết nối");
                  }
                } catch {
                  toast.error("Kiểm tra proxy thất bại");
                }
              }}
            >
              <LuSparkles className="size-2.5 text-primary" />
              <span>
                {t("proxies.form.checkProxy", { defaultValue: "Test" })}
              </span>
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-2 text-[10px] text-destructive hover:bg-destructive/10 hover:text-destructive gap-1 ml-auto"
              onClick={() => {
                void onSelectProxy(profile.id, null);
                onClose();
              }}
            >
              <LuTrash2 className="size-2.5" />
              <span>{t("proxies.remove", { defaultValue: "Gỡ" })}</span>
            </Button>
          </div>
        </div>
      )}

      <div className="grid grid-cols-2 gap-1 rounded-lg bg-muted p-1 text-[11px]">
        <button
          type="button"
          onClick={() => setActiveTab("paste")}
          className={cn(
            "rounded-md py-1 font-medium transition-all",
            activeTab === "paste"
              ? "bg-background text-foreground shadow-xs"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {t("proxies.quickInput", { defaultValue: "Nhập / Dán IP mới" })}
        </button>
        <button
          type="button"
          onClick={() => setActiveTab("saved")}
          className={cn(
            "rounded-md py-1 font-medium transition-all",
            activeTab === "saved"
              ? "bg-background text-foreground shadow-xs"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {t("createProfile.proxy.savedTab", { defaultValue: "Proxy đã lưu" })}{" "}
          ({storedProxies.length})
        </button>
      </div>

      {activeTab === "paste" ? (
        <div className="space-y-2 pt-1">
          <div className="relative">
            <Input
              value={proxyInput}
              onChange={(e) => setProxyInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void handleApplyRawProxy();
                }
              }}
              placeholder="host:port:user:pass hoặc http://..."
              className="h-8 font-mono text-xs pr-16"
              autoFocus
            />
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={handlePasteClipboard}
              className="absolute right-1 top-1 h-6 px-1.5 text-[10px] text-muted-foreground hover:text-foreground"
            >
              <LuCopy className="size-2.5 mr-1" />
              {t("common.buttons.paste", { defaultValue: "Dán" })}
            </Button>
          </div>
          <div className="flex justify-end gap-1.5">
            <Button
              variant="outline"
              size="sm"
              className="h-7 text-xs"
              onClick={onClose}
            >
              {t("common.buttons.cancel", { defaultValue: "Hủy" })}
            </Button>
            <Button
              size="sm"
              className="h-7 text-xs"
              disabled={!proxyInput.trim() || isApplying}
              onClick={handleApplyRawProxy}
            >
              {isApplying
                ? t("common.checking", { defaultValue: "Đang lưu..." })
                : t("common.buttons.apply", { defaultValue: "Áp dụng" })}
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-1.5 pt-1">
          <Input
            value={searchFilter}
            onChange={(e) => setSearchFilter(e.target.value)}
            placeholder={t("createProfile.proxy.searchSaved", {
              defaultValue: "Tìm proxy đã lưu...",
            })}
            className="h-7 text-xs"
          />
          <div className="max-h-36 overflow-y-auto space-y-0.5 pr-1">
            <button
              type="button"
              onClick={() => {
                void onSelectProxy(profile.id, null);
                onClose();
              }}
              className={cn(
                "w-full flex items-center justify-between rounded px-2 py-1.5 text-left text-xs transition-colors hover:bg-accent",
                !effectiveProxyId && "bg-accent/60 font-medium text-primary",
              )}
            >
              <span>
                {t("createProfile.proxy.noProxy", {
                  defaultValue: "Không dùng proxy (None)",
                })}
              </span>
              {!effectiveProxyId && (
                <LuCheck className="size-3.5 text-primary" />
              )}
            </button>
            {filteredStored.map((p) => (
              <button
                key={p.id}
                type="button"
                onClick={() => {
                  void onSelectProxy(profile.id, p.id);
                  onClose();
                }}
                className={cn(
                  "w-full flex items-center justify-between rounded px-2 py-1.5 text-left font-mono text-xs transition-colors hover:bg-accent",
                  effectiveProxyId === p.id &&
                    "bg-accent/60 font-semibold text-primary",
                )}
              >
                <span className="truncate">
                  {p.name ||
                    `${p.proxy_settings.host}:${p.proxy_settings.port}`}
                </span>
                {effectiveProxyId === p.id && (
                  <LuCheck className="size-3.5 text-primary shrink-0" />
                )}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

// Must be rendered inside a <Popover>; the tooltip shows the full assignment
// name only when it is truncated in the cell.
const ProxyCellTrigger = React.memo<{
  displayName: string;
  hasAssignment: boolean;
  isDisabled: boolean;
  assignedProxy?: StoredProxy | null;
}>(({ displayName, hasAssignment, isDisabled, assignedProxy }) => {
  const textRef = React.useRef<HTMLSpanElement | null>(null);
  const [isOverflowing, setIsOverflowing] = React.useState(false);

  return (
    <div className="group/proxy flex max-w-full min-w-0 items-center gap-1">
      <Tooltip
        onOpenChange={(open) => {
          if (!open) return;
          const el = textRef.current;
          if (el) setIsOverflowing(el.scrollWidth > el.clientWidth);
        }}
      >
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <span
              className={cn(
                "group/cell flex h-6 max-w-full min-w-0 items-center gap-1.5 rounded-md px-2 py-1 transition-colors duration-100",
                isDisabled
                  ? "pointer-events-none cursor-not-allowed opacity-60"
                  : "cursor-pointer hover:bg-foreground/6",
              )}
            >
              {hasAssignment ? (
                <span
                  ref={textRef}
                  className="min-w-0 truncate text-sm font-mono tracking-tight"
                >
                  {displayName}
                </span>
              ) : (
                <EmptyCellValue label={displayName} />
              )}
            </span>
          </PopoverTrigger>
        </TooltipTrigger>
        {hasAssignment && isOverflowing && (
          <TooltipContent>{displayName}</TooltipContent>
        )}
      </Tooltip>

      {assignedProxy && !isDisabled && (
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                const str = formatProxyClipboard(assignedProxy);
                void navigator.clipboard.writeText(str);
                toast.success(`Đã sao chép: ${str}`);
              }}
              className="opacity-0 group-hover/proxy:opacity-100 transition-opacity p-1 text-muted-foreground hover:text-foreground rounded hover:bg-accent"
            >
              <LuCopy className="size-3" />
            </button>
          </TooltipTrigger>
          <TooltipContent side="top">Sao chép Proxy</TooltipContent>
        </Tooltip>
      )}
    </div>
  );
});

ProxyCellTrigger.displayName = "ProxyCellTrigger";

const NoteCell = React.memo<{
  profile: BrowserProfile;
  isDisabled: boolean;
  noteOverrides: Record<string, string | null>;
  openNoteEditorFor: string | null;
  setOpenNoteEditorFor: React.Dispatch<React.SetStateAction<string | null>>;
  setNoteOverrides: React.Dispatch<
    React.SetStateAction<Record<string, string | null>>
  >;
}>(
  ({
    profile,
    isDisabled,
    noteOverrides,
    openNoteEditorFor,
    setOpenNoteEditorFor,
    setNoteOverrides,
  }) => {
    const { t } = useTranslation();
    const _profileDrag = useProfileGroupDrag();
    const effectiveNote: string | null = Object.hasOwn(
      noteOverrides,
      profile.id,
    )
      ? noteOverrides[profile.id]
      : (profile.note ?? null);

    const onNoteChange = React.useCallback(
      async (newNote: string | null) => {
        const trimmedNote = newNote?.trim() ?? null;
        setNoteOverrides((prev) => ({ ...prev, [profile.id]: trimmedNote }));
        try {
          await invoke<BrowserProfile>("update_profile_note", {
            profileId: profile.id,
            note: trimmedNote,
          });
        } catch (error) {
          console.error("Failed to update note:", error);
          setNoteOverrides((prev) => {
            const next = { ...prev };
            delete next[profile.id];
            return next;
          });
          toast.error(error instanceof Error ? error.message : String(error));
        }
      },
      [profile.id, setNoteOverrides],
    );

    const editorRef = React.useRef<HTMLDivElement | null>(null);
    const textareaRef = React.useRef<HTMLTextAreaElement | null>(null);
    const [noteValue, setNoteValue] = React.useState(effectiveNote ?? "");

    // Update local state when effective note changes (from outside)
    React.useEffect(() => {
      if (openNoteEditorFor !== profile.id) {
        setNoteValue(effectiveNote ?? "");
      }
    }, [effectiveNote, openNoteEditorFor, profile.id]);

    // Auto-resize textarea on open
    React.useEffect(() => {
      if (openNoteEditorFor === profile.id && textareaRef.current) {
        const textarea = textareaRef.current;
        textarea.style.height = "auto";
        textarea.style.height = `${Math.min(textarea.scrollHeight, 200)}px`;
      }
    }, [openNoteEditorFor, profile.id]);

    const handleTextareaChange = React.useCallback(
      (e: React.ChangeEvent<HTMLTextAreaElement>) => {
        const newValue = e.target.value;
        setNoteValue(newValue);
        // Auto-resize
        const textarea = e.target;
        textarea.style.height = "auto";
        textarea.style.height = `${Math.min(textarea.scrollHeight, 200)}px`;
      },
      [],
    );

    React.useEffect(() => {
      if (openNoteEditorFor !== profile.id) return;
      const handleClick = (e: MouseEvent) => {
        const target = e.target as Node | null;
        if (
          editorRef.current &&
          target &&
          !editorRef.current.contains(target)
        ) {
          const currentValue = textareaRef.current?.value ?? "";
          void onNoteChange(currentValue);
          setOpenNoteEditorFor(null);
        }
      };
      document.addEventListener("mousedown", handleClick);
      return () => {
        document.removeEventListener("mousedown", handleClick);
      };
    }, [openNoteEditorFor, profile.id, setOpenNoteEditorFor, onNoteChange]);

    React.useEffect(() => {
      if (openNoteEditorFor === profile.id && textareaRef.current) {
        textareaRef.current.focus();
        // Move cursor to end
        const len = textareaRef.current.value.length;
        textareaRef.current.setSelectionRange(len, len);
      }
    }, [openNoteEditorFor, profile.id]);

    const displayNote = effectiveNote ?? "";
    const showTooltip = displayNote.length > 0;

    if (openNoteEditorFor !== profile.id) {
      return (
        <div className="min-h-6 w-full">
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                className={cn(
                  "group/cell flex min-h-6 w-full min-w-0 items-center rounded-md border-none bg-transparent px-2 py-1 text-left transition-colors duration-100",
                  isDisabled
                    ? "cursor-not-allowed opacity-60"
                    : "cursor-pointer hover:bg-foreground/6",
                )}
                onClick={() => {
                  if (!isDisabled) {
                    setNoteValue(effectiveNote ?? "");
                    setOpenNoteEditorFor(profile.id);
                  }
                }}
              >
                {effectiveNote ? (
                  <span className="block w-full truncate text-sm">
                    {displayNote}
                  </span>
                ) : (
                  <EmptyCellValue
                    hint={isDisabled ? null : t("profiles.note.placeholder")}
                    label={t("profiles.note.empty")}
                  />
                )}
              </button>
            </TooltipTrigger>
            {showTooltip && (
              <TooltipContent className="max-w-[320px]">
                <p className="wrap-break-word whitespace-pre-wrap">
                  {effectiveNote ?? t("profiles.note.empty")}
                </p>
              </TooltipContent>
            )}
          </Tooltip>
        </div>
      );
    }

    return (
      <div
        className={cn(
          "relative w-full",
          isDisabled && "pointer-events-none opacity-60",
        )}
      >
        <div
          ref={editorRef}
          className="absolute top-[-15px] -left-px z-50 min-h-6 w-60 rounded-md border bg-popover shadow-md"
        >
          <textarea
            ref={textareaRef}
            value={noteValue}
            onChange={handleTextareaChange}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                setNoteValue(effectiveNote ?? "");
                setOpenNoteEditorFor(null);
              } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                void onNoteChange(noteValue);
                setOpenNoteEditorFor(null);
              }
            }}
            onBlur={() => {
              void onNoteChange(noteValue);
              setOpenNoteEditorFor(null);
            }}
            placeholder={t("profiles.note.placeholder")}
            className="max-h-[200px] min-h-6 w-full resize-none border-0 bg-transparent px-2 py-1 text-sm focus:ring-0 focus:outline-none"
            style={{
              overflow: "auto",
            }}
            rows={1}
          />
        </div>
      </div>
    );
  },
);

NoteCell.displayName = "NoteCell";

interface ProfilesDataTableProps {
  workspaceId: string | null;
  profiles: BrowserProfile[];
  storedProxies: StoredProxy[];
  onLaunchProfile: (profile: BrowserProfile) => void | Promise<void>;
  onKillProfile: (profile: BrowserProfile) => void | Promise<void>;
  onCloneProfile: (profile: BrowserProfile) => void | Promise<void>;
  onShareProfile?: (profile: BrowserProfile) => void;
  onDeleteProfile: (profile: BrowserProfile) => void | Promise<void>;
  onRenameProfile: (profileId: string, newName: string) => Promise<void>;
  onConfigureWayfern: (profile: BrowserProfile) => void;
  onCopyCookiesToProfile?: (profile: BrowserProfile) => void;
  onOpenCookieManagement?: (profile: BrowserProfile) => void;
  runningProfiles: Set<string>;
  isUpdating: (browser: string) => boolean;
  onDeleteSelectedProfiles: (profileIds: string[]) => Promise<void>;
  onAssignProfilesToGroup: (profileIds: string[]) => void;
  selectedGroupId: string | null;
  selectedProfiles: string[];
  onSelectedProfilesChange: Dispatch<SetStateAction<string[]>>;
  onBulkDelete?: () => void;
  onBulkGroupAssignment?: () => void;
  onBulkProxyAssignment?: () => void;
  onBulkCopyCookies?: () => void;
  onBulkRun?: () => void;
  onBulkStop?: () => void;
  onBulkShare?: () => void;
  bulkActionsUnlocked?: boolean;
  onBulkExtensionGroupAssignment?: () => void;
  onAssignExtensionGroup?: (profileIds: string[]) => void;
  crossOsUnlocked?: boolean;
  syncUnlocked?: boolean;
  getProfileSyncInfo?: (profileId: string) =>
    | {
        session: SyncSessionInfo;
        isLeader: boolean;
        failedAtUrl: string | null;
      }
    | undefined;
  onLaunchWithSync?: (profile: BrowserProfile) => void;
  onSetPassword?: (profile: BrowserProfile) => void;
  onChangePassword?: (profile: BrowserProfile) => void;
  onRemovePassword?: (profile: BrowserProfile) => void;
  /**
   * When provided, the info dialog is controlled by the parent. Allows the
   * command palette in page.tsx to open the dialog directly without lifting
   * every other piece of internal table state.
   */
  infoDialogProfile?: BrowserProfile | null;
  onInfoDialogProfileChange?: (profile: BrowserProfile | null) => void;
  /** Initial data load in flight — renders skeleton rows instead of "empty". */
  isLoading?: boolean;
  /** True when the app has zero profiles overall (not just a filtered view). */
  showOnboardingEmptyState?: boolean;
  onCreateProfile?: () => void;
  onRefreshProfiles?: () => Promise<void>;
  isRefreshingProfiles?: boolean;
  workspaceSelected?: boolean;
  filterControls?: React.ReactNode;
  groups?: Array<{ id: string; name: string; profile_count?: number }>;
  totalProfilesCount?: number;
  onOpenGroupManagement?: () => void;
  onOpenProxyManagement?: () => void;
  onOpenMemberManagement?: () => void;
  workspaceName?: string;
  workspaceRole?: string;
  organizationName?: string;
  maxProfiles?: number;
  onGroupSelect?: (groupId: string) => void;
}

export function ProfilesDataTable({
  workspaceId,
  profiles,
  storedProxies,
  onLaunchProfile,
  onKillProfile,
  onCloneProfile,
  onShareProfile,
  onDeleteProfile,
  onRenameProfile,
  onConfigureWayfern,
  onCopyCookiesToProfile,
  onOpenCookieManagement,
  runningProfiles,
  isUpdating,
  onAssignProfilesToGroup,
  selectedGroupId,
  selectedProfiles,
  onSelectedProfilesChange,
  onBulkDelete,
  onBulkGroupAssignment,
  onBulkProxyAssignment,
  onBulkCopyCookies,
  onBulkRun,
  onBulkStop,
  onBulkShare,
  bulkActionsUnlocked = false,
  onBulkExtensionGroupAssignment,
  onAssignExtensionGroup,
  crossOsUnlocked = false,
  syncUnlocked = false,
  getProfileSyncInfo,
  onLaunchWithSync,
  onSetPassword,
  onChangePassword,
  onRemovePassword,
  infoDialogProfile,
  onInfoDialogProfileChange,
  isLoading = false,
  showOnboardingEmptyState = false,
  onCreateProfile,
  onRefreshProfiles,
  isRefreshingProfiles = false,
  workspaceSelected,
  filterControls,
  groups = [],
  totalProfilesCount,
  onGroupSelect,
  onOpenGroupManagement,
  onOpenProxyManagement,
  onOpenMemberManagement,
  workspaceName,
  workspaceRole,
  organizationName,
  maxProfiles,
}: ProfilesDataTableProps) {
  const { t } = useTranslation();
  const profileDrag = useProfileGroupDrag();
  const view = useTablePreferences("profiles");
  const { getTableSorting, updateSorting, isLoaded } = useTableSorting();
  const [sorting, setSorting] = React.useState<SortingState>([]);

  React.useEffect(() => {
    if (view.loaded && view.preferences.sorting) {
      setSorting(view.preferences.sorting);
    }
  }, [view.loaded, view.preferences.sorting]);

  // Sync external selectedProfiles with table's row selection state
  const [rowSelection, setRowSelection] = React.useState<RowSelectionState>({});
  const prevSelectedProfilesRef = React.useRef<string[]>(selectedProfiles);

  // Update row selection when external selectedProfiles changes
  React.useEffect(() => {
    // Only update if selectedProfiles actually changed
    if (
      prevSelectedProfilesRef.current.length !== selectedProfiles.length ||
      !prevSelectedProfilesRef.current.every((id) =>
        selectedProfiles.includes(id),
      )
    ) {
      const newSelection: RowSelectionState = {};
      for (const profileId of selectedProfiles) {
        newSelection[profileId] = true;
      }
      setRowSelection(newSelection);
      prevSelectedProfilesRef.current = selectedProfiles;
      // When the parent clears the selection (e.g. after a bulk action like
      // delete / move-to-group), collapse the checkbox column back to icons.
      // Otherwise the row checkboxes stay visible and only revert after the
      // user clicks one — which the per-checkbox handler resets.
      if (selectedProfiles.length === 0) {
        setShowCheckboxes(false);
      }
    }
  }, [selectedProfiles]);

  // Update external selectedProfiles when table selection changes
  const handleRowSelectionChange = React.useCallback(
    (updater: React.SetStateAction<RowSelectionState>) => {
      setRowSelection((prevSelection) => {
        const newSelection =
          typeof updater === "function" ? updater(prevSelection) : updater;

        const selectedIds = Object.keys(newSelection).filter(
          (id) => newSelection[id],
        );

        // Only update external state if selection actually changed.
        // A Set gives O(1) membership; Array.includes() inside .every() would
        // be O(n*m) over large selections.
        const prevIdSet = new Set(
          Object.keys(prevSelection).filter((id) => prevSelection[id]),
        );

        if (
          selectedIds.length !== prevIdSet.size ||
          !selectedIds.every((id) => prevIdSet.has(id))
        ) {
          onSelectedProfilesChange(selectedIds);
        }

        return newSelection;
      });
    },
    [onSelectedProfilesChange],
  );
  const [profileToRename, setProfileToRename] =
    React.useState<BrowserProfile | null>(null);
  const [newProfileName, setNewProfileName] = React.useState("");
  const [renameError, setRenameError] = React.useState<string | null>(null);
  const [isRenamingSaving, setIsRenamingSaving] = React.useState(false);
  const renameContainerRef = React.useRef<HTMLDivElement | null>(null);
  const [profileToDelete, setProfileToDelete] =
    React.useState<BrowserProfile | null>(null);
  const [isDeleting, setIsDeleting] = React.useState(false);
  const [internalInfoDialogProfile, setInternalInfoDialogProfile] =
    React.useState<BrowserProfile | null>(null);
  const isInfoDialogControlled = onInfoDialogProfileChange !== undefined;
  const profileForInfoDialog = isInfoDialogControlled
    ? (infoDialogProfile ?? null)
    : internalInfoDialogProfile;
  const setProfileForInfoDialog = React.useCallback(
    (p: BrowserProfile | null) => {
      if (isInfoDialogControlled) {
        onInfoDialogProfileChange?.(p);
      } else {
        setInternalInfoDialogProfile(p);
      }
    },
    [isInfoDialogControlled, onInfoDialogProfileChange],
  );
  const [bypassRulesProfile, setBypassRulesProfile] =
    React.useState<BrowserProfile | null>(null);
  const [dnsBlocklistProfile, setDnsBlocklistProfile] =
    React.useState<BrowserProfile | null>(null);
  const [launchHookProfile, setLaunchHookProfile] =
    React.useState<BrowserProfile | null>(null);
  const [launchingProfiles, setLaunchingProfiles] = React.useState<Set<string>>(
    new Set(),
  );
  const [stoppingProfiles, setStoppingProfiles] = React.useState<Set<string>>(
    new Set(),
  );

  const { user } = useCloudAuth();
  const {
    isProfileLocked,
    getLockInfo,
    runtimeRunningProfileIds,
    refetchLocks,
  } = useTeamLocks(user?.id, workspaceId, profiles, runningProfiles);

  const [proxyOverrides, setProxyOverrides] = React.useState<
    Record<string, string | null>
  >({});
  const [showCheckboxes, setShowCheckboxes] = React.useState(false);
  const [tagsOverrides, setTagsOverrides] = React.useState<
    Record<string, string[]>
  >({});
  const [allTags, setAllTags] = React.useState<string[]>([]);
  const [openTagsEditorFor, setOpenTagsEditorFor] = React.useState<
    string | null
  >(null);
  const [openProxySelectorFor, setOpenProxySelectorFor] = React.useState<
    string | null
  >(null);
  const [checkingProfileId, setCheckingProfileId] = React.useState<
    string | null
  >(null);
  const [proxyCheckResults, setProxyCheckResults] = React.useState<
    Record<string, ProxyCheckResult>
  >({});
  const [noteOverrides, setNoteOverrides] = React.useState<
    Record<string, string | null>
  >({});
  const [openNoteEditorFor, setOpenNoteEditorFor] = React.useState<
    string | null
  >(null);
  const [trafficSnapshots, setTrafficSnapshots] = React.useState<
    Record<string, TrafficSnapshot>
  >({});
  const [trafficDialogProfile, setTrafficDialogProfile] = React.useState<{
    id: string;
    name?: string;
  } | null>(null);
  const [syncStatuses, setSyncStatuses] = React.useState<
    Record<
      string,
      {
        status: string;
        error?: string;
        generation?: number;
        can_open?: boolean;
        can_open_error?: string;
        checkedAt?: number;
        source?: "api";
      }
    >
  >({});
  const syncStatusesRef = React.useRef(syncStatuses);
  syncStatusesRef.current = syncStatuses;
  const syncStatusRequests = React.useRef(new Set<string>());
  const syncStatusRevisions = React.useRef(new Map<string, number>());

  // Country proxy creation state (for inline proxy creation in dropdown)
  const [countries, setCountries] = React.useState<LocationItem[]>([]);
  const [countriesLoaded, setCountriesLoaded] = React.useState(false);

  // Extension group lookup is provided by the desktop API, not legacy Tauri
  // commands.
  const [extensionGroups, setExtensionGroups] = React.useState<
    ExtensionGroup[]
  >([]);

  React.useEffect(() => {
    let active = true;
    if (!workspaceId) {
      setExtensionGroups([]);
      return;
    }
    void invoke<ExtensionGroup[]>("list_extension_groups", { workspaceId })
      .then((groups) => {
        if (active) setExtensionGroups(groups);
      })
      .catch((error) => {
        if (active) {
          // Keep the backend Problem fields visible in the first console line;
          // DevTools otherwise collapses the object and hides the actionable
          // auth, workspace, or capability error behind this generic prefix.
          const problem =
            error && typeof error === "object"
              ? (error as Record<string, unknown>)
              : {};
          const context = [
            typeof problem.code === "string" ? problem.code : null,
            typeof problem.status === "number"
              ? `HTTP ${problem.status}`
              : null,
            typeof problem.correlationId === "string"
              ? `correlationId ${problem.correlationId}`
              : null,
          ]
            .filter(Boolean)
            .join(", ");
          console.error(
            `Failed to load extension groups: ${extractRootError(error)}${context ? ` (${context})` : ""}`,
            error,
          );
          setExtensionGroups([]);
        }
      });
    return () => {
      active = false;
    };
  }, [workspaceId]);

  const canCreateLocationProxy = false;

  const loadCountries = React.useCallback(async () => {
    if (countriesLoaded || !canCreateLocationProxy) return;
    try {
      const data = await invoke<LocationItem[]>("cloud_get_countries");
      setCountries(data);
      setCountriesLoaded(true);
    } catch (e) {
      console.error("Failed to load countries:", e);
    }
  }, [countriesLoaded]);

  // Load cached check results for proxies
  React.useEffect(() => {
    const loadCachedResults = async () => {
      const results: Record<string, ProxyCheckResult> = {};
      const proxyIds = new Set<string>();
      for (const profile of profiles) {
        if (profile.proxy_id) {
          proxyIds.add(profile.proxy_id);
        }
      }
      for (const proxyId of proxyIds) {
        try {
          const cached = await invoke<ProxyCheckResult | null>(
            "get_cached_proxy_check",
            { proxyId },
          );
          if (cached) {
            results[proxyId] = cached;
          }
        } catch (_error) {
          // Ignore errors
        }
      }
      setProxyCheckResults(results);
    };
    if (profiles.length > 0) {
      void loadCachedResults();
    }
  }, [profiles]);

  const loadAllTags = React.useCallback(async () => {
    const tags = new Set<string>();
    for (const profile of profiles) {
      for (const tag of profile.tags ?? []) {
        const normalized = tag.trim();
        if (normalized) tags.add(normalized);
      }
    }
    setAllTags([...tags].sort((a, b) => a.localeCompare(b)));
  }, [profiles]);

  const handleProxySelection = React.useCallback(
    async (profileId: string, proxyId: string | null) => {
      try {
        await invoke("update_profile_proxy", {
          profileId,
          proxyId,
        });
        setProxyOverrides((prev) => ({ ...prev, [profileId]: proxyId }));
        await emit("profile-updated");
      } catch (error) {
        console.error("Failed to update proxy settings:", error);
      } finally {
        setOpenProxySelectorFor(null);
      }
    },
    [],
  );

  const handleCreateCountryProxy = React.useCallback(
    async (profileId: string, country: LocationItem) => {
      try {
        await invoke("create_cloud_location_proxy", {
          name: country.name,
          country: country.code,
          region: null,
          city: null,
          isp: null,
        });
        await emit("stored-proxies-changed");
        // Wait briefly for proxy list to update, then find and assign the new proxy
        await new Promise((r) => setTimeout(r, 200));
        const updatedProxies =
          await invoke<StoredProxy[]>("get_stored_proxies");
        const newProxy = updatedProxies.find(
          (p: StoredProxy) =>
            p.is_cloud_derived && p.geo_country === country.code,
        );
        if (newProxy) {
          await handleProxySelection(profileId, newProxy.id);
        }
        setOpenProxySelectorFor(null);
      } catch (error) {
        console.error("Failed to create country proxy:", error);
      }
    },
    [handleProxySelection],
  );

  // Use shared browser state hook
  const browserState = useBrowserState(
    profiles,
    runningProfiles,
    isUpdating,
    launchingProfiles,
    stoppingProfiles,
  );

  const refreshProfileSyncStatus = React.useCallback(
    async (profileId: string, force = false) => {
      const profile = profiles.find((item) => item.id === profileId);
      if (!profile) return;
      const managed =
        profile.sync_mode === "managed" ||
        profile.sync_mode === "Encrypted" ||
        profile.sync_mode === "Regular";
      if (!managed) return;
      const current = syncStatusesRef.current[profileId];
      if (
        syncStatusRequests.current.has(profileId) ||
        (!force &&
          current?.status === "syncing" &&
          runningProfiles.has(profileId)) ||
        (!force &&
          current?.source === "api" &&
          current.checkedAt &&
          Date.now() - current.checkedAt < 30_000)
      ) {
        return;
      }
      if (!force && runningProfiles.has(profileId)) {
        if (current?.status === "waiting") return;
        setSyncStatuses((previous) => ({
          ...previous,
          [profileId]: {
            status: "waiting",
            checkedAt: Date.now(),
          },
        }));
        return;
      }
      syncStatusRequests.current.add(profileId);
      const revision = syncStatusRevisions.current.get(profileId) ?? 0;
      try {
        const remote = await invoke<{
          status: string;
          generation?: number | null;
          can_open: boolean;
          can_open_error?: string | null;
        }>("profile_sync_status_get", { profileId });
        if ((syncStatusRevisions.current.get(profileId) ?? 0) !== revision)
          return;
        setSyncStatuses((previous) => ({
          ...previous,
          [profileId]: {
            status: remote.status,
            error: remote.can_open_error
              ? formatError(remote.can_open_error)
              : undefined,
            generation: remote.generation ?? undefined,
            can_open: remote.can_open,
            can_open_error: remote.can_open_error ?? undefined,
            checkedAt: Date.now(),
            source: "api",
          },
        }));
      } catch (error) {
        if ((syncStatusRevisions.current.get(profileId) ?? 0) !== revision)
          return;
        console.error("Failed to refresh profile sync status:", error);
        setSyncStatuses((previous) => ({
          ...previous,
          [profileId]: {
            status: "error",
            error: formatError(error),
            checkedAt: Date.now(),
            source: "api",
          },
        }));
      } finally {
        syncStatusRequests.current.delete(profileId);
      }
    },
    [profiles, runningProfiles],
  );

  React.useEffect(() => {
    if (!browserState.isClient) return;
    const refresh = () => {
      for (const profile of profiles) {
        if (
          profile.sync_mode === "managed" ||
          profile.sync_mode === "Encrypted" ||
          profile.sync_mode === "Regular"
        ) {
          void refreshProfileSyncStatus(profile.id);
        }
      }
    };
    refresh();
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") refresh();
    }, 30_000);
    const refreshWhenVisible = () => {
      if (document.visibilityState === "visible") refresh();
    };
    window.addEventListener("focus", refreshWhenVisible);
    document.addEventListener("visibilitychange", refreshWhenVisible);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", refreshWhenVisible);
      document.removeEventListener("visibilitychange", refreshWhenVisible);
    };
  }, [browserState.isClient, profiles, refreshProfileSyncStatus]);

  // Listen for sync status events
  React.useEffect(() => {
    if (!browserState.isClient) return;
    const unlisten = listen("profile-fingerprint-warning", () => {
      showErrorToast(t("profileTable.nativeFingerprintWarning"));
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [browserState.isClient, t]);

  React.useEffect(() => {
    if (!browserState.isClient) return;
    const unlisten = listen<{ profileId: string; warnings: string[] }>(
      "profile-extension-warning",
      ({ payload }) => {
        showErrorToast(
          t("extensions.launchUpdateWarning", {
            count: payload.warnings.length,
          }),
          { description: payload.warnings.join("\n") },
        );
      },
    );
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [browserState.isClient, t]);

  React.useEffect(() => {
    if (!browserState.isClient) return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{
          profile_id: string;
          status: string;
          error?: string;
        }>("profile-sync-status", (event) => {
          const { profile_id, status, error } = event.payload;
          syncStatusRevisions.current.set(
            profile_id,
            (syncStatusRevisions.current.get(profile_id) ?? 0) + 1,
          );
          setSyncStatuses((prev) => ({
            ...prev,
            [profile_id]: { status, error, checkedAt: Date.now() },
          }));
          // A failed stop/sync event is authoritative for the local attempt.
          // Refreshing immediately can replace its error with the server's
          // still-old manifest status (`waiting`) and hide the actual failure.
          if (status === "synced") {
            void refreshProfileSyncStatus(profile_id, true);
          }
        });
      } catch (error) {
        console.error("Failed to listen for sync status events:", error);
      }
    })();
    return () => {
      if (unlisten) unlisten();
    };
  }, [browserState.isClient, refreshProfileSyncStatus]);

  // Fetch traffic snapshots for running profiles (lightweight, real-time data)
  // Convert Set to sorted array to avoid Set reference comparison issues in dependencies
  const runningProfileIds = React.useMemo(
    () => Array.from(runningProfiles).sort(),
    [runningProfiles],
  );
  const runningCount = runningProfileIds.length;
  React.useEffect(() => {
    if (!browserState.isClient) return;

    if (runningCount === 0) {
      setTrafficSnapshots({});
      return;
    }

    const fetchTrafficSnapshots = async () => {
      try {
        const allSnapshots = await invoke<TrafficSnapshot[]>(
          "get_all_traffic_snapshots",
        );
        const newSnapshots: Record<string, TrafficSnapshot> = {};
        // O(1) membership; runningProfileIds.includes() in this loop would be
        // O(snapshots * runningProfiles).
        const runningSet = new Set(runningProfileIds);
        for (const snapshot of allSnapshots) {
          if (snapshot.profile_id) {
            // Only keep snapshots for profiles that are currently running
            if (runningSet.has(snapshot.profile_id)) {
              const existing = newSnapshots[snapshot.profile_id];
              if (!existing || snapshot.last_update > existing.last_update) {
                newSnapshots[snapshot.profile_id] = snapshot;
              }
            }
          }
        }
        setTrafficSnapshots(newSnapshots);
      } catch (error) {
        console.error("Failed to fetch traffic snapshots:", error);
      }
    };

    void fetchTrafficSnapshots();
    const interval = setInterval(() => {
      void fetchTrafficSnapshots();
    }, 1000);
    return () => {
      clearInterval(interval);
    };
  }, [browserState.isClient, runningCount, runningProfileIds]);

  // Clean up snapshots for profiles that are no longer running
  React.useEffect(() => {
    if (!browserState.isClient) return;

    setTrafficSnapshots((prev) => {
      const cleaned: Record<string, TrafficSnapshot> = {};
      const runningSet = new Set(runningProfileIds);
      for (const [profileId, snapshot] of Object.entries(prev)) {
        // Only keep snapshots for profiles that are currently running
        if (runningSet.has(profileId)) {
          cleaned[profileId] = snapshot;
        }
      }
      // Only update if something was removed
      if (Object.keys(cleaned).length !== Object.keys(prev).length) {
        return cleaned;
      }
      return prev;
    });
  }, [browserState.isClient, runningProfileIds]);

  // Clear launching/stopping spinners when backend reports running status changes
  React.useEffect(() => {
    if (!browserState.isClient) return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ id: string; is_running: boolean }>(
          "profile-running-changed",
          (event) => {
            const { id } = event.payload;
            // Clear launching state for this profile if present
            setLaunchingProfiles((prev) => {
              if (!prev.has(id)) return prev;
              const next = new Set(prev);
              next.delete(id);
              return next;
            });
            // Clear stopping state for this profile if present
            setStoppingProfiles((prev) => {
              if (!prev.has(id)) return prev;
              const next = new Set(prev);
              next.delete(id);
              return next;
            });
          },
        );
      } catch (error) {
        console.error("Failed to listen for profile running changes:", error);
      }
    })();
    return () => {
      if (unlisten) unlisten();
    };
  }, [browserState.isClient]);

  // Keep stored proxies up-to-date by listening for changes emitted elsewhere in the app
  React.useEffect(() => {
    if (!browserState.isClient) return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen("stored-proxies-changed", () => {
          // Also refresh tags on profile updates
          void loadAllTags();
        });
      } catch (_err) {
        // Best-effort only
      }
    })();
    return () => {
      if (unlisten) unlisten();
    };
  }, [browserState.isClient, loadAllTags]);

  // A running browser keeps the name it launched with, so close any inline
  // rename that was open when the profile entered a runtime transition.
  React.useEffect(() => {
    if (!profileToRename) return;

    const profileId = profileToRename.id;
    const isRuntimeLocked =
      (browserState.isClient && runningProfiles.has(profileId)) ||
      launchingProfiles.has(profileId) ||
      stoppingProfiles.has(profileId);

    if (isRuntimeLocked) {
      setProfileToRename(null);
      setNewProfileName("");
      setRenameError(null);
    }
  }, [
    profileToRename,
    runningProfiles,
    launchingProfiles,
    stoppingProfiles,
    browserState.isClient,
  ]);

  // Automatically deselect profiles that become running, updating, launching, or stopping
  React.useEffect(() => {
    const newSet = new Set(selectedProfiles);
    let hasChanges = false;

    for (const profileId of selectedProfiles) {
      const profile = profiles.find((p) => p.id === profileId);
      if (profile) {
        const isRunning =
          browserState.isClient && runningProfiles.has(profile.id);
        const isLaunching = launchingProfiles.has(profile.id);
        const isStopping = stoppingProfiles.has(profile.id);

        if (isRunning || isLaunching || isStopping) {
          newSet.delete(profileId);
          hasChanges = true;
        }
      }
    }

    if (hasChanges) {
      onSelectedProfilesChange(Array.from(newSet));
    }
  }, [
    profiles,
    runningProfiles,
    launchingProfiles,
    stoppingProfiles,
    browserState.isClient,
    onSelectedProfilesChange,
    selectedProfiles,
  ]);

  // Update local sorting state when settings are loaded
  React.useEffect(() => {
    if (isLoaded && browserState.isClient) {
      setSorting(getTableSorting());
    }
  }, [isLoaded, getTableSorting, browserState.isClient]);

  // Handle sorting changes
  const handleSortingChange = React.useCallback(
    (updater: React.SetStateAction<SortingState>) => {
      if (!browserState.isClient) return;
      const newSorting =
        typeof updater === "function" ? updater(sorting) : updater;
      setSorting(newSorting);
      updateSorting(newSorting);
      view.setSorting(newSorting);
    },
    [browserState.isClient, sorting, updateSorting, view],
  );

  const handleRename = React.useCallback(async () => {
    if (!profileToRename || !newProfileName.trim()) return;

    try {
      setIsRenamingSaving(true);
      await onRenameProfile(profileToRename.id, newProfileName.trim());
      setProfileToRename(null);
      setNewProfileName("");
      setRenameError(null);
    } catch (error) {
      setRenameError(
        error instanceof Error
          ? error.message
          : t("errors.renameProfileFailed", { error: String(error) }),
      );
    } finally {
      setIsRenamingSaving(false);
    }
  }, [profileToRename, newProfileName, onRenameProfile, t]);

  // Cancel inline rename on outside click
  React.useEffect(() => {
    if (!profileToRename) return;
    const handleClickOutside = (event: MouseEvent) => {
      const target = event.target as Node | null;
      if (
        target &&
        renameContainerRef.current &&
        !renameContainerRef.current.contains(target)
      ) {
        setProfileToRename(null);
        setNewProfileName("");
        setRenameError(null);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, [profileToRename]);

  const handleDelete = async () => {
    if (!profileToDelete) return;

    setIsDeleting(true);
    // Minimum loading time for visual feedback
    const minLoadingTime = new Promise((r) => setTimeout(r, 300));
    try {
      await Promise.all([onDeleteProfile(profileToDelete), minLoadingTime]);
      setProfileToDelete(null);
    } catch (error) {
      console.error("Failed to delete profile:", error);
    } finally {
      setIsDeleting(false);
    }
  };

  // Handle icon/checkbox click
  const handleIconClick = React.useCallback(
    (profileId: string) => {
      const profile = profiles.find((p) => p.id === profileId);
      if (!profile) return;

      // Prevent selection of profiles whose browsers are updating
      if (!browserState.canSelectProfile(profile)) {
        return;
      }

      setShowCheckboxes(true);
      const newSet = new Set(selectedProfiles);
      if (newSet.has(profileId)) {
        newSet.delete(profileId);
      } else {
        newSet.add(profileId);
      }

      // Hide checkboxes if no profiles are selected
      if (newSet.size === 0) {
        setShowCheckboxes(false);
      }

      onSelectedProfilesChange(Array.from(newSet));
    },
    [profiles, browserState, onSelectedProfilesChange, selectedProfiles],
  );

  React.useEffect(() => {
    if (browserState.isClient) {
      void loadAllTags();
    }
  }, [browserState.isClient, loadAllTags]);

  // Handle checkbox change
  const handleCheckboxChange = React.useCallback(
    (profileId: string, checked: boolean) => {
      const newSet = new Set(selectedProfiles);
      if (checked) {
        newSet.add(profileId);
      } else {
        newSet.delete(profileId);
      }

      // Hide checkboxes if no profiles are selected
      if (newSet.size === 0) {
        setShowCheckboxes(false);
      }

      onSelectedProfilesChange(Array.from(newSet));
    },
    [onSelectedProfilesChange, selectedProfiles],
  );

  // Handle select all checkbox
  const handleToggleAll = React.useCallback(
    (checked: boolean) => {
      const newSet = checked
        ? new Set(
            profiles
              .filter((profile) => {
                const isRunning =
                  browserState.isClient && runningProfiles.has(profile.id);
                const isLaunching = launchingProfiles.has(profile.id);
                const isStopping = stoppingProfiles.has(profile.id);
                return !isRunning && !isLaunching && !isStopping;
              })
              .map((profile) => profile.id),
          )
        : new Set<string>();

      setShowCheckboxes(checked);
      onSelectedProfilesChange(Array.from(newSet));
    },
    [
      profiles,
      onSelectedProfilesChange,
      browserState.isClient,
      runningProfiles,
      launchingProfiles,
      stoppingProfiles,
    ],
  );

  // Memoize selectableProfiles calculation
  const selectableProfiles = React.useMemo(() => {
    return profiles.filter((profile) => {
      const isRunning =
        browserState.isClient && runningProfiles.has(profile.id);
      const isLaunching = launchingProfiles.has(profile.id);
      const isStopping = stoppingProfiles.has(profile.id);
      return !isRunning && !isLaunching && !isStopping;
    });
  }, [
    profiles,
    browserState.isClient,
    runningProfiles,
    launchingProfiles,
    stoppingProfiles,
  ]);

  // Build table meta from volatile state so columns can stay stable
  const tableMeta = React.useMemo<TableMeta>(
    () => ({
      workspaceId,
      t,
      selectedProfiles,
      selectableCount: selectableProfiles.length,
      showCheckboxes,
      isClient: browserState.isClient,
      runningProfiles,
      runtimeRunningProfileIds,
      launchingProfiles,
      stoppingProfiles,
      isUpdating,
      browserState,

      // Tags editor state
      tagsOverrides,
      allTags,
      openTagsEditorFor,
      setAllTags,
      setOpenTagsEditorFor,
      setTagsOverrides,

      // Note editor state
      noteOverrides,
      openNoteEditorFor,
      setOpenNoteEditorFor,
      setNoteOverrides,

      // Proxy selector state
      openProxySelectorFor,
      setOpenProxySelectorFor,
      proxyOverrides,
      storedProxies,
      handleProxySelection,
      checkingProfileId,
      proxyCheckResults,

      // Extension groups
      extensionGroups,
      onAssignExtensionGroup,
      setDnsBlocklistProfile,

      // Selection helpers
      isProfileSelected: (id: string) => selectedProfiles.includes(id),
      selectedDragProfiles: profiles.filter((p) =>
        selectedProfiles.includes(p.id),
      ),
      canDragProfiles: (dragProfiles: BrowserProfile[]) =>
        dragProfiles.every(
          (p) =>
            !runningProfiles.has(p.id) &&
            !launchingProfiles.has(p.id) &&
            !stoppingProfiles.has(p.id),
        ),
      handleToggleAll,
      handleCheckboxChange,
      handleIconClick,

      // Rename helpers
      handleRename,
      setProfileToRename,
      setNewProfileName,
      setRenameError,
      profileToRename,
      newProfileName,
      isRenamingSaving,
      renameError,

      // Launch/stop helpers
      setLaunchingProfiles,
      setStoppingProfiles,
      onKillProfile,
      onLaunchProfile,

      // Overflow actions
      onAssignProfilesToGroup,
      onCloneProfile: onCloneProfile
        ? (profile: BrowserProfile) => {
            void onCloneProfile(profile);
          }
        : undefined,
      onShareProfile,
      onConfigureWayfern,
      onCopyCookiesToProfile,
      onOpenCookieManagement,

      // Traffic snapshots (lightweight real-time data)
      trafficSnapshots,
      onOpenTrafficDialog: (profileId: string) => {
        const profile = profiles.find((p) => p.id === profileId);
        setTrafficDialogProfile({ id: profileId, name: profile?.name });
      },

      // Sync
      syncStatuses,
      refreshProfileSyncStatus,
      crossOsUnlocked,
      syncUnlocked,

      // Country proxy creation
      countries,
      canCreateLocationProxy,
      loadCountries,
      handleCreateCountryProxy,

      // Team locks
      isProfileLockedByAnother: isProfileLocked,
      getProfileLockEmail: (profileId: string) =>
        getLockInfo(profileId)?.ownerLabel ??
        getLockInfo(profileId)?.lockedByEmail,
      getProfileLockInfo: getLockInfo,
      refreshProfileLockStatus: refetchLocks,

      // Synchronizer
      getProfileSyncInfo: getProfileSyncInfo ?? (() => undefined),
      onLaunchWithSync:
        onLaunchWithSync ??
        (() => {
          /* empty */
        }),
    }),
    [
      workspaceId,
      t,
      selectedProfiles,
      selectableProfiles.length,
      showCheckboxes,
      browserState.isClient,
      runningProfiles,
      runtimeRunningProfileIds,
      launchingProfiles,
      stoppingProfiles,
      isUpdating,
      browserState,
      tagsOverrides,
      allTags,
      openTagsEditorFor,
      noteOverrides,
      openNoteEditorFor,
      openProxySelectorFor,
      proxyOverrides,
      storedProxies,
      handleProxySelection,
      checkingProfileId,
      proxyCheckResults,
      extensionGroups,
      onAssignExtensionGroup,
      handleToggleAll,
      handleCheckboxChange,
      handleIconClick,
      handleRename,
      profileToRename,
      newProfileName,
      isRenamingSaving,
      trafficSnapshots,
      profiles,
      renameError,
      onKillProfile,
      onLaunchProfile,
      onAssignProfilesToGroup,
      onCloneProfile,
      onShareProfile,
      onConfigureWayfern,
      onCopyCookiesToProfile,
      onOpenCookieManagement,
      syncStatuses,
      refreshProfileSyncStatus,
      crossOsUnlocked,
      syncUnlocked,
      countries,
      loadCountries,
      handleCreateCountryProxy,
      isProfileLocked,
      getLockInfo,
      refetchLocks,
      getProfileSyncInfo,
      onLaunchWithSync,
    ],
  );

  const columns: ColumnDef<BrowserProfile>[] = React.useMemo(
    () => [
      {
        id: "select",
        header: ({ table }) => <TableSelectHeader table={table} />,
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original;
          const crossOs = isCrossOsProfile(profile);
          const os = profile.host_os || profile.wayfern_config?.os || "";
          const Icon = crossOs ? getOSIcon(os) : getProfileIcon(profile);
          const message = crossOs
            ? t("crossOs.viewOnly", { os: getOSDisplayName(os) })
            : meta.runningProfiles.has(profile.id)
              ? t("profiles.table.cantModifyRunning")
              : meta.launchingProfiles.has(profile.id)
                ? t("profiles.table.cantModifyLaunching")
                : meta.stoppingProfiles.has(profile.id)
                  ? t("profiles.table.cantModifyStopping")
                  : getBrowserDisplayName(profile.browser);
          return (
            <NonHoverableTooltip
              content={<p>{message}</p>}
              sideOffset={4}
              horizontalOffset={8}
            >
              <span className="block">
                <TableRowCheckbox
                  table={table}
                  row={row}
                  icon={
                    Icon ? (
                      <Icon
                        className={cn(
                          "size-4",
                          !row.getCanSelect() && "opacity-50",
                        )}
                      />
                    ) : undefined
                  }
                />
              </span>
            </NonHoverableTooltip>
          );
        },
        enableSorting: false,
        enableHiding: false,
        enableResizing: false,
        size: 28,
      },
      {
        id: "actions",
        size: 44,
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original;
          const handoff = meta.getRemoteHandoff
            ? meta.getRemoteHandoff(profile.id)
            : null;
          const isRunningRemotely =
            handoff === "running" ||
            (meta.runtimeRunningProfileIds.has(profile.id) &&
              !meta.runningProfiles.has(profile.id));
          const isPendingRemotePull = handoff === "pending_sync";
          const isRunning =
            meta.isClient && meta.runningProfiles.has(profile.id);
          const isLaunching = meta.launchingProfiles.has(profile.id);
          const isStopping = meta.stoppingProfiles.has(profile.id);
          const isLockedByAnother = meta.isProfileLockedByAnother(profile.id);
          const syncEntry = meta.syncStatuses[profile.id];
          const requiresSyncKey =
            profile.sync_mode === "managed" ||
            profile.sync_mode === "Encrypted" ||
            profile.sync_mode === "Regular";
          const canOpenHere = !requiresSyncKey || syncEntry?.can_open !== false;
          const canLaunch =
            meta.browserState.canLaunchProfile(profile) && !isPendingRemotePull;
          const lockEmail = meta.getProfileLockEmail(profile.id);
          const lockInfo = meta.getProfileLockInfo(profile.id);
          const expiryLabel = lockInfo?.expiresAt
            ? meta.t("sync.team.leaseExpiry", {
                time: new Date(lockInfo.expiresAt).toLocaleString(),
              })
            : "";
          const lockOwnerLabel = `${lockEmail ?? ""}${expiryLabel}`;
          const tooltipContent = isPendingRemotePull
            ? meta.t("profiles.remote.pendingSyncTooltip")
            : isRunningRemotely || isLockedByAnother
              ? meta.t("sync.team.launchWillRecheck", {
                  owner: lockOwnerLabel || meta.t("sync.team.anotherDevice"),
                })
              : meta.browserState.getLaunchTooltipContent(profile);
          const openTooltipContent =
            syncEntry?.can_open === false
              ? formatError(
                  syncEntry.can_open_error ?? "sync_key_recovery_required",
                )
              : syncEntry?.status === "error"
                ? (syncEntry.error ?? tooltipContent)
                : tooltipContent;

          const handleProfileStop = async (profile: BrowserProfile) => {
            meta.setStoppingProfiles((prev: Set<string>) =>
              new Set(prev).add(profile.id),
            );
            try {
              await meta.onKillProfile(profile);
            } catch (error) {
              meta.setStoppingProfiles((prev: Set<string>) => {
                const next = new Set(prev);
                next.delete(profile.id);
                return next;
              });
              throw error;
            } finally {
              void meta.refreshProfileLockStatus();
            }
          };

          const handleProfileLaunch = async (profile: BrowserProfile) => {
            meta.setLaunchingProfiles((prev: Set<string>) =>
              new Set(prev).add(profile.id),
            );
            try {
              await meta.onLaunchProfile(profile);
            } finally {
              meta.setLaunchingProfiles((prev: Set<string>) => {
                const next = new Set(prev);
                next.delete(profile.id);
                return next;
              });
              void meta.refreshProfileLockStatus();
            }
          };

          const syncInfo = meta.getProfileSyncInfo(profile.id);
          const isLeader = syncInfo?.isLeader === true;
          const isFollower = syncInfo?.isLeader === false;
          const isDesynced = isFollower && syncInfo.failedAtUrl != null;
          const stopTooltip = isLeader
            ? meta.t("profiles.synchronizer.stopLeader")
            : isFollower
              ? meta.t("profiles.synchronizer.stopFollower", {
                  leaderName: syncInfo.session.leader_profile_name ?? "",
                })
              : tooltipContent;

          const handleStop = async () => {
            if (isLeader && syncInfo) {
              try {
                await invoke("stop_sync_session", {
                  sessionId: syncInfo.session.id,
                });
              } catch (error) {
                console.error("Failed to stop sync session:", error);
              }
            } else if (isFollower && syncInfo) {
              try {
                await invoke("remove_sync_follower", {
                  sessionId: syncInfo.session.id,
                  followerProfileId: profile.id,
                });
              } catch (error) {
                console.error("Failed to remove sync follower:", error);
              }
            } else {
              await handleProfileStop(profile);
            }
          };

          return (
            <div className="flex items-center gap-2">
              {isDesynced && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span>
                      <LuTriangleAlert className="size-4 text-warning" />
                    </span>
                  </TooltipTrigger>
                  <TooltipContent>
                    {meta.t("profiles.synchronizer.desyncedTooltip", {
                      url: syncInfo?.failedAtUrl ?? "",
                    })}
                  </TooltipContent>
                </Tooltip>
              )}
              {(isRunning || canOpenHere) && (
                <LaunchControlTooltip
                  launching={isLaunching}
                  content={isRunning ? stopTooltip : openTooltipContent}
                >
                  <span className="inline-flex">
                    <RippleButton
                      variant="soft"
                      size="sm"
                      data-running={isRunning ? "true" : undefined}
                      disabled={!canLaunch || isLaunching || isStopping}
                      aria-busy={isLaunching || isStopping}
                      aria-label={
                        isLaunching
                          ? meta.t("profileSelector.launching")
                          : isRunning
                            ? meta.t("profiles.actions.stop")
                            : isRunningRemotely || isLockedByAnother
                              ? tooltipContent
                              : meta.t("profiles.actions.launch")
                      }
                      className={cn(
                        "grid size-7 place-items-center rounded-full p-0 transition-[background-color,color,transform] duration-150",
                        !canLaunch && "cursor-not-allowed opacity-50",
                        canLaunch && "cursor-pointer",
                        !isRunning &&
                          "hover:bg-primary hover:text-primary-foreground",
                        isRunning &&
                          !isFollower &&
                          "bg-destructive/12 text-destructive hover:bg-destructive/20",
                      )}
                      onClick={() =>
                        isRunning
                          ? void handleStop()
                          : void handleProfileLaunch(profile)
                      }
                    >
                      <span className="grid place-items-center">
                        {isLaunching || isStopping ? (
                          <span className="size-3 animate-spin rounded-full border border-current border-t-transparent motion-reduce:animate-none" />
                        ) : isRunning ? (
                          <LuSquare className="size-3 fill-current" />
                        ) : (
                          <LuPlay className="size-3 translate-x-px fill-current" />
                        )}
                      </span>
                    </RippleButton>
                  </span>
                </LaunchControlTooltip>
              )}
            </div>
          );
        },
      },
      {
        // Hidden, sort-only column so profiles can be sorted by creation date
        // without showing a Created column in the table. Kept
        // hidden via columnVisibility; sorting still works on hidden columns.
        id: "created_at",
        accessorFn: (row) => row.created_at ?? 0,
        enableSorting: true,
        enableHiding: true,
        sortingFn: "basic",
        header: () => null,
        cell: () => null,
      },
      {
        accessorKey: "name",
        // The only column without a fixed width: table-fixed hands it all
        // remaining space as the window grows or shrinks.
        meta: { flexWidth: true },
        // The Name header doubles as the sort control: clicking opens a menu to
        // sort by name (A–Z / Z–A) or by creation date (newest / oldest), so
        // creation-date sorting needs no visible column.
        header: ({ table }) => {
          const meta = table.options.meta as TableMeta;
          // The legacy renderer's context carries the core table, which does
          // not expose the hook wrapper's subscribed state. Close over the
          // component state instead so sort indicators render in v9.
          const sort = sorting[0];
          const isActive = (id: string, desc: boolean) =>
            sort?.id === id && !!sort.desc === desc;
          return (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="subtle"
                  className="-ml-1.5 h-6 justify-start gap-1 rounded-md px-1.5 text-left text-xs font-medium"
                >
                  {meta.t("common.labels.name")}
                  {isActive("name", false) ? (
                    <LuChevronUp className="size-3" />
                  ) : isActive("name", true) ? (
                    <LuChevronDown className="size-3" />
                  ) : (
                    <LuChevronDown className="size-3 opacity-60" />
                  )}
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start">
                <DropdownMenuItem
                  onClick={() =>
                    table.setSorting([{ id: "name", desc: false }])
                  }
                >
                  {isActive("name", false) && (
                    <LuCheck className="mr-2 size-3.5" />
                  )}
                  {meta.t("profiles.sort.nameAsc")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() => table.setSorting([{ id: "name", desc: true }])}
                >
                  {isActive("name", true) && (
                    <LuCheck className="mr-2 size-3.5" />
                  )}
                  {meta.t("profiles.sort.nameDesc")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() =>
                    table.setSorting([{ id: "created_at", desc: true }])
                  }
                >
                  {isActive("created_at", true) && (
                    <LuCheck className="mr-2 size-3.5" />
                  )}
                  {meta.t("profiles.sort.newest")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() =>
                    table.setSorting([{ id: "created_at", desc: false }])
                  }
                >
                  {isActive("created_at", false) && (
                    <LuCheck className="mr-2 size-3.5" />
                  )}
                  {meta.t("profiles.sort.oldest")}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          );
        },
        enableSorting: true,
        sortingFn: "alphanumeric",
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original as BrowserProfile;
          const rawName: string = row.getValue("name");
          const name = getBrowserDisplayName(rawName);
          const isRuntimeLocked =
            (meta.isClient && meta.runningProfiles.has(profile.id)) ||
            meta.launchingProfiles.has(profile.id) ||
            meta.stoppingProfiles.has(profile.id);
          const isCrossOsBlocked = isCrossOsProfile(profile);
          const isEditing =
            meta.profileToRename?.id === profile.id &&
            !isRuntimeLocked &&
            !isCrossOsBlocked;

          if (isEditing) {
            return (
              <div
                ref={renameContainerRef}
                className="relative overflow-visible"
              >
                <Input
                  autoFocus
                  value={meta.newProfileName}
                  onChange={(e) => {
                    meta.setNewProfileName(e.target.value);
                    if (meta.renameError) meta.setRenameError(null);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !(e.metaKey || e.ctrlKey)) {
                      void meta.handleRename();
                    } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                      void meta.handleRename();
                    } else if (e.key === "Escape") {
                      meta.setProfileToRename(null);
                      meta.setNewProfileName("");
                      meta.setRenameError(null);
                    }
                  }}
                  onBlur={() => {
                    if (
                      meta.newProfileName.trim().length > 0 &&
                      meta.newProfileName.trim() !== profile.name
                    ) {
                      void meta.handleRename();
                    } else {
                      meta.setProfileToRename(null);
                      meta.setNewProfileName("");
                      meta.setRenameError(null);
                    }
                  }}
                  className="h-6 w-full max-w-full min-w-0 border-0 px-2 py-1 text-sm leading-none font-medium shadow-none focus-visible:ring-0"
                />
              </div>
            );
          }

          const display = (
            <OverflowTooltipText
              text={name}
              className="text-left leading-none font-medium"
            />
          );

          const lockedEmail = meta.getProfileLockEmail(profile.id);
          const isLocked = meta.isProfileLockedByAnother(profile.id);
          const lockInfo = meta.getProfileLockInfo(profile.id);
          const expiryLabel = lockInfo?.expiresAt
            ? meta.t("sync.team.leaseExpiry", {
                time: new Date(lockInfo.expiresAt).toLocaleString(),
              })
            : "";
          const dragProfiles = meta.selectedProfiles.includes(profile.id)
            ? meta.selectedDragProfiles
            : [profile];
          const canDrag = () => meta.canDragProfiles(dragProfiles);
          const nameControl = isRuntimeLocked ? (
            <div className="mr-auto h-6 max-w-full min-w-0 cursor-text overflow-hidden rounded px-2 py-1 text-left select-text">
              {display}
            </div>
          ) : (
            <button
              type="button"
              className={cn(
                "mr-auto h-6 max-w-full min-w-0 overflow-hidden rounded-md border-none bg-transparent px-2 py-1 text-left transition-colors duration-100",
                isCrossOsBlocked
                  ? "cursor-not-allowed opacity-60"
                  : "cursor-pointer hover:bg-foreground/6",
              )}
              onClick={() => {
                if (isCrossOsBlocked) return;
                meta.setProfileToRename(profile);
                meta.setNewProfileName(profile.name);
                meta.setRenameError(null);
              }}
              onKeyDown={(e) => {
                if (isCrossOsBlocked) return;
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  meta.setProfileToRename(profile);
                  meta.setNewProfileName(profile.name);
                  meta.setRenameError(null);
                }
              }}
            >
              {display}
            </button>
          );

          return (
            <div className="-ml-1 flex max-w-full min-w-0 items-center gap-0.5">
              <ProfileGroupDragHandle
                profile={profile}
                profiles={dragProfiles}
                disabled={!canDrag()}
                canMove={meta.canDragProfiles}
                onChooseGroup={() =>
                  meta.onAssignProfilesToGroup?.(
                    dragProfiles.map((target) => target.id),
                  )
                }
              />
              {nameControl}
              <ProfileHandoffStatus
                profileId={profile.id}
                ownerLabel={meta.getProfileLockEmail(profile.id)}
                state={(() => {
                  const state = meta.getRemoteHandoff?.(profile.id);
                  return state === "running" || state === "pending_sync"
                    ? state
                    : null;
                })()}
                syncStatus={meta.syncStatuses[profile.id]}
                compact
                onOpenSync={
                  meta.onOpenProfileSyncDialog
                    ? () => meta.onOpenProfileSyncDialog?.(profile)
                    : undefined
                }
              />
              {isLocked && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span>
                      <LuLock className="size-3 text-muted-foreground" />
                    </span>
                  </TooltipTrigger>
                  <TooltipContent>
                    {meta.t("sync.team.profileLocked", {
                      owner: `${lockedEmail ?? ""}${expiryLabel}`,
                    })}
                  </TooltipContent>
                </Tooltip>
              )}
            </div>
          );
        },
      },
      {
        id: "tags",
        size: 100,
        header: ({ table }) => {
          const meta = table.options.meta as TableMeta;
          return meta.t("profileTable.tagsHeader");
        },
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original;
          const isDisabled = isCrossOsProfile(profile);

          return (
            <TagsCell
              profile={profile}
              isDisabled={isDisabled}
              tagsOverrides={meta.tagsOverrides ?? {}}
              allTags={meta.allTags ?? []}
              setAllTags={meta.setAllTags}
              openTagsEditorFor={meta.openTagsEditorFor ?? null}
              setOpenTagsEditorFor={meta.setOpenTagsEditorFor}
              setTagsOverrides={meta.setTagsOverrides}
            />
          );
        },
      },
      {
        id: "note",
        size: 80,
        header: ({ table }) => {
          const meta = table.options.meta as TableMeta;
          return meta.t("profileTable.noteHeader");
        },
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original;
          const isDisabled = isCrossOsProfile(profile);

          return (
            <NoteCell
              profile={profile}
              isDisabled={isDisabled}
              noteOverrides={meta.noteOverrides ?? {}}
              openNoteEditorFor={meta.openNoteEditorFor ?? null}
              setOpenNoteEditorFor={meta.setOpenNoteEditorFor}
              setNoteOverrides={meta.setNoteOverrides}
            />
          );
        },
      },
      {
        id: "proxy",
        size: 110,
        header: ({ table }) => {
          const meta = table.options.meta as TableMeta;
          return meta.t("profiles.table.proxy");
        },
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original;
          const isCrossOs = isCrossOsProfile(profile);
          const isCrossOsBlocked = isCrossOs;
          const isRunning =
            meta.isClient && meta.runningProfiles.has(profile.id);
          const isLaunching = meta.launchingProfiles.has(profile.id);
          const isStopping = meta.stoppingProfiles.has(profile.id);
          const isDisabled =
            isRunning || isLaunching || isStopping || isCrossOsBlocked;

          const hasProxyOverride = Object.hasOwn(
            meta.proxyOverrides,
            profile.id,
          );
          const effectiveProxyId = hasProxyOverride
            ? meta.proxyOverrides[profile.id]
            : (profile.proxy_id ?? null);
          const effectiveProxy = effectiveProxyId
            ? (meta.storedProxies.find((p) => p.id === effectiveProxyId) ??
              null)
            : null;

          const hasAssignment = Boolean(effectiveProxy);
          const displayName = effectiveProxy
            ? effectiveProxy.name
            : meta.t("profiles.table.notSelected");
          const isSelectorOpen = meta.openProxySelectorFor === profile.id;
          const _selectedId = effectiveProxyId;

          // When profile is running:
          // 1. If NO proxy assigned, show clean dash (direct connection, no fake proxy chart)
          // 2. If proxy assigned AND active traffic snapshot exists, show bandwidth chart
          // 3. If proxy assigned but no active snapshot yet, show the assigned proxy with active status
          if (isRunning) {
            if (!hasAssignment) {
              return (
                <div className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground/60 select-none">
                  <span>—</span>
                </div>
              );
            }

            const snapshot = meta.trafficSnapshots?.[profile.id];
            const hasTraffic = Boolean(
              snapshot &&
                ((snapshot.current_bytes_sent ?? 0) > 0 ||
                  (snapshot.current_bytes_received ?? 0) > 0 ||
                  (snapshot.recent_bandwidth &&
                    snapshot.recent_bandwidth.length > 0)),
            );

            if (hasTraffic && snapshot) {
              const bandwidthData = snapshot.recent_bandwidth
                ? [...snapshot.recent_bandwidth]
                : [];
              const currentBandwidth =
                (snapshot.current_bytes_sent ?? 0) +
                (snapshot.current_bytes_received ?? 0);

              return (
                <div className="min-w-0 overflow-hidden">
                  <BandwidthMiniChart
                    key={`${profile.id}-${snapshot.last_update ?? 0}-${bandwidthData.length}`}
                    data={bandwidthData}
                    currentBandwidth={currentBandwidth}
                    onClick={() => meta.onOpenTrafficDialog?.(profile.id)}
                  />
                </div>
              );
            }

            return (
              <div className="flex min-w-0 items-center gap-2 overflow-hidden text-xs">
                <span className="relative flex h-2 w-2 shrink-0">
                  <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75" />
                  <span className="relative inline-flex rounded-full h-2 w-2 bg-emerald-500" />
                </span>
                <span className="truncate font-medium text-foreground/90">
                  {displayName}
                </span>
              </div>
            );
          }

          return (
            <div className="flex min-w-0 items-center gap-2 overflow-hidden">
              <Popover
                open={isSelectorOpen}
                onOpenChange={(open) => {
                  meta.setOpenProxySelectorFor(open ? profile.id : null);
                }}
              >
                <ProxyCellTrigger
                  displayName={displayName}
                  hasAssignment={hasAssignment}
                  isDisabled={isDisabled}
                  assignedProxy={effectiveProxy}
                />

                {!isDisabled && (
                  <PopoverContent
                    className="w-auto p-0 border-border bg-popover text-popover-foreground shadow-xl"
                    align="start"
                    sideOffset={8}
                  >
                    <SmartProxyPopoverContent
                      profile={profile}
                      effectiveProxy={effectiveProxy}
                      effectiveProxyId={effectiveProxyId}
                      storedProxies={meta.storedProxies}
                      onSelectProxy={meta.handleProxySelection}
                      onClose={() => meta.setOpenProxySelectorFor(null)}
                    />
                  </PopoverContent>
                )}
              </Popover>
              {effectiveProxy && !isDisabled && (
                <ProxyCheckButton
                  proxy={effectiveProxy}
                  profileId={profile.id}
                  checkingProfileId={meta.checkingProfileId}
                  cachedResult={meta.proxyCheckResults[effectiveProxy.id]}
                  setCheckingProfileId={setCheckingProfileId}
                  onCheckComplete={(result) => {
                    setProxyCheckResults((prev) => ({
                      ...prev,
                      [effectiveProxy.id]: result,
                    }));
                  }}
                  onCheckFailed={(result) => {
                    setProxyCheckResults((prev) => ({
                      ...prev,
                      [effectiveProxy.id]: result,
                    }));
                  }}
                />
              )}
            </div>
          );
        },
      },
      {
        id: "ext",
        size: 95,
        header: ({ table }) => {
          const meta = table.options.meta as TableMeta;
          return meta.t("profiles.table.ext");
        },
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original;
          return <ExtCell profile={profile} meta={meta} />;
        },
      },
      {
        id: "dns",
        size: 95,
        header: ({ table }) => {
          const meta = table.options.meta as TableMeta;
          return meta.t("profiles.table.dns");
        },
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original;
          return <DnsCell profile={profile} meta={meta} />;
        },
      },
      {
        id: "sync",
        header: "",
        size: 28,
        enableSorting: false,
        enableHiding: false,
        enableResizing: false,
        cell: ({ row, table }) => {
          const profile = row.original;
          const meta = table.options.meta as TableMeta;
          const syncEntry = meta.syncStatuses[profile.id];
          // A remote runtime projection can outlive the upload if its final
          // stopped update failed. Only the local sync event/API result may
          // paint this dot amber; a lease projection alone is not sync activity.
          const liveStatus = syncEntry?.status as
            | "syncing"
            | "waiting"
            | "synced"
            | "error"
            | "disabled"
            | undefined;

          const dot = getProfileSyncStatusDot(
            profile,
            liveStatus,
            meta.t,
            syncEntry?.error,
          );
          if (!dot) return null;

          return (
            <Tooltip>
              <TooltipTrigger asChild>
                <span className="flex h-9 w-full items-center justify-center">
                  {dot.encrypted ? (
                    <LuLock
                      className={`size-3 ${dot.color.replace("bg-", "text-")}${dot.animate ? " animate-pulse" : ""}`}
                    />
                  ) : (
                    <span
                      className={`size-2 rounded-full ${dot.color}${dot.animate ? " animate-pulse" : ""}`}
                    />
                  )}
                </span>
              </TooltipTrigger>
              <TooltipContent>{dot.tooltip}</TooltipContent>
            </Tooltip>
          );
        },
      },
      {
        id: "settings",
        header: "",
        size: 104,
        enableSorting: false,
        enableHiding: false,
        enableResizing: false,
        cell: ({ row, table }) => {
          const meta = table.options.meta as TableMeta;
          const profile = row.original;
          const isRunning =
            meta.isClient && meta.runningProfiles.has(profile.id);
          const isLaunching = meta.launchingProfiles.has(profile.id);
          const isStopping = meta.stoppingProfiles.has(profile.id);
          const isBusy = isLaunching || isStopping;
          const hasProxyOverride = Object.hasOwn(
            meta.proxyOverrides,
            profile.id,
          );
          const effectiveProxyId = hasProxyOverride
            ? meta.proxyOverrides[profile.id]
            : (profile.proxy_id ?? null);
          const effectiveProxy = effectiveProxyId
            ? (meta.storedProxies.find((p) => p.id === effectiveProxyId) ??
              null)
            : null;

          return (
            <div className="flex h-9 w-full items-center justify-end gap-1 pr-1">
              {meta.onShareProfile && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="subtle"
                      size="icon"
                      className="size-7 rounded-md p-0 text-muted-foreground hover:text-foreground"
                      aria-label={meta.t("profileSharing.open")}
                      onClick={() => meta.onShareProfile?.(profile)}
                    >
                      <LuUsers className="size-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>
                    {meta.t("profileSharing.open")}
                  </TooltipContent>
                </Tooltip>
              )}

              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="subtle"
                    data-slot="profile-inspect-trigger"
                    className="size-7 rounded-md p-0 text-muted-foreground hover:text-foreground"
                    disabled={!meta.isClient}
                    onClick={() => {
                      setProfileForInfoDialog(profile);
                    }}
                  >
                    <span className="sr-only">
                      {t("profiles.aria.profileInfo")}
                    </span>
                    <LuInfo className="size-4" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  {t("profiles.actions.info", {
                    defaultValue: "Chi tiết Profile",
                  })}
                </TooltipContent>
              </Tooltip>

              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="subtle"
                    className="size-7 rounded-md p-0 text-muted-foreground hover:text-foreground data-[state=open]:bg-foreground/10"
                    disabled={!meta.isClient || isBusy}
                  >
                    <span className="sr-only">
                      {t("common.labels.actions", { defaultValue: "Thao tác" })}
                    </span>
                    <LuEllipsis className="size-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="end"
                  className="w-56 p-1.5 rounded-xl border border-border/80 bg-popover/95 backdrop-blur-md shadow-xl text-xs font-sans"
                >
                  <DropdownMenuLabel className="px-2 py-1 text-[10px] font-semibold tracking-wider text-muted-foreground/70 uppercase">
                    {t("common.labels.actions", { defaultValue: "Thao tác" })}
                  </DropdownMenuLabel>

                  {/* Launch / Stop */}
                  {isRunning ? (
                    <DropdownMenuItem
                      onClick={() => void meta.onKillProfile(profile)}
                      className="gap-2.5 px-2.5 py-1.5 rounded-lg text-amber-500 focus:text-amber-500 focus:bg-amber-500/10 font-medium cursor-pointer"
                    >
                      <LuSquare className="size-4 shrink-0" />
                      <span>
                        {t("profiles.actions.stop", {
                          defaultValue: "Dừng Profile",
                        })}
                      </span>
                    </DropdownMenuItem>
                  ) : (
                    <DropdownMenuItem
                      onClick={() => void meta.onLaunchProfile(profile)}
                      className="gap-2.5 px-2.5 py-1.5 rounded-lg text-emerald-500 focus:text-emerald-500 focus:bg-emerald-500/10 font-medium cursor-pointer"
                    >
                      <LuPlay className="size-4 shrink-0" />
                      <span>
                        {t("profiles.actions.launch", {
                          defaultValue: "Khởi chạy",
                        })}
                      </span>
                    </DropdownMenuItem>
                  )}

                  {meta.onShareProfile && (
                    <DropdownMenuItem
                      onClick={() => meta.onShareProfile?.(profile)}
                      className="gap-2.5 px-2.5 py-1.5 rounded-lg cursor-pointer"
                    >
                      <LuUsers className="size-4 shrink-0 text-muted-foreground" />
                      <span>{meta.t("profileSharing.open")}</span>
                    </DropdownMenuItem>
                  )}

                  <DropdownMenuSeparator className="my-1 bg-border/50" />

                  {/* Proxy actions */}
                  <DropdownMenuLabel className="px-2 py-1 text-[10px] font-semibold tracking-wider text-muted-foreground/70 uppercase">
                    Proxy / Mạng
                  </DropdownMenuLabel>

                  <DropdownMenuItem
                    onClick={() => {
                      meta.setOpenProxySelectorFor(profile.id);
                    }}
                    className="gap-2.5 px-2.5 py-1.5 rounded-lg cursor-pointer"
                  >
                    <LuGlobe className="size-4 shrink-0 text-muted-foreground" />
                    <span>
                      {effectiveProxy
                        ? t("proxies.edit", { defaultValue: "Đổi Proxy" })
                        : t("proxies.assign", { defaultValue: "Gán Proxy" })}
                    </span>
                  </DropdownMenuItem>

                  {effectiveProxy && (
                    <>
                      <DropdownMenuItem
                        onClick={() => {
                          const str = formatProxyClipboard(effectiveProxy);
                          navigator.clipboard.writeText(str);
                          toast.success(`Đã sao chép: ${str}`);
                        }}
                        className="gap-2.5 px-2.5 py-1.5 rounded-lg cursor-pointer"
                      >
                        <LuCopy className="size-4 shrink-0 text-muted-foreground" />
                        <span>
                          {t("proxies.copyString", {
                            defaultValue: "Sao chép Proxy (host:port)",
                          })}
                        </span>
                      </DropdownMenuItem>

                      <DropdownMenuItem
                        onClick={async () => {
                          toast.info("Đang kiểm tra kết nối...");
                          try {
                            const res = await invoke<ProxyCheckResult>(
                              "check_proxy_validity",
                              { proxySettings: effectiveProxy.proxy_settings },
                            );
                            if (res.is_valid) {
                              toast.success(`Proxy hoạt động (${res.ip})`);
                            } else {
                              toast.error("Proxy không thể kết nối");
                            }
                          } catch (err: unknown) {
                            showErrorToast(
                              formatError(err) || "Kiểm tra thất bại",
                            );
                          }
                        }}
                        className="gap-2.5 px-2.5 py-1.5 rounded-lg cursor-pointer"
                      >
                        <LuSparkles className="size-4 shrink-0 text-primary" />
                        <span>
                          {t("proxies.testProxy", {
                            defaultValue: "Kiểm tra Proxy",
                          })}
                        </span>
                      </DropdownMenuItem>

                      <DropdownMenuItem
                        onClick={() => {
                          void meta.handleProxySelection(profile.id, null);
                          toast.success(
                            t("proxies.detached", {
                              defaultValue: "Đã gỡ proxy",
                            }),
                          );
                        }}
                        className="gap-2.5 px-2.5 py-1.5 rounded-lg text-destructive focus:text-destructive focus:bg-destructive/10 cursor-pointer"
                      >
                        <LuTrash2 className="size-4 shrink-0" />
                        <span>
                          {t("proxies.remove", { defaultValue: "Gỡ Proxy" })}
                        </span>
                      </DropdownMenuItem>
                    </>
                  )}

                  <DropdownMenuSeparator className="my-1 bg-border/50" />

                  {/* Cookies / Group / Clone */}
                  <DropdownMenuLabel className="px-2 py-1 text-[10px] font-semibold tracking-wider text-muted-foreground/70 uppercase">
                    Hồ sơ & Dữ liệu
                  </DropdownMenuLabel>

                  {meta.onCopyCookiesToProfile && (
                    <DropdownMenuItem
                      onClick={() => meta.onCopyCookiesToProfile?.(profile)}
                      className="gap-2.5 px-2.5 py-1.5 rounded-lg cursor-pointer"
                    >
                      <LuCookie className="size-4 shrink-0 text-muted-foreground" />
                      <span>
                        {t("profiles.actions.copyCookies", {
                          defaultValue: "Sao chép Cookies",
                        })}
                      </span>
                    </DropdownMenuItem>
                  )}

                  {meta.onCloneProfile && (
                    <DropdownMenuItem
                      onClick={() => meta.onCloneProfile?.(profile)}
                      className="gap-2.5 px-2.5 py-1.5 rounded-lg cursor-pointer"
                    >
                      <LuCopy className="size-4 shrink-0 text-muted-foreground" />
                      <span>
                        {t("profiles.actions.clone", {
                          defaultValue: "Nhân bản (Clone)",
                        })}
                      </span>
                    </DropdownMenuItem>
                  )}

                  {meta.onAssignProfilesToGroup && (
                    <DropdownMenuItem
                      onClick={() =>
                        meta.onAssignProfilesToGroup?.([profile.id])
                      }
                      className="gap-2.5 px-2.5 py-1.5 rounded-lg cursor-pointer"
                    >
                      <LuFolder className="size-4 shrink-0 text-muted-foreground" />
                      <span>
                        {t("profiles.actions.assignGroup", {
                          defaultValue: "Gán vào Nhóm",
                        })}
                      </span>
                    </DropdownMenuItem>
                  )}

                  <DropdownMenuItem
                    onClick={() => {
                      setProfileForInfoDialog(profile);
                    }}
                    className="gap-2.5 px-2.5 py-1.5 rounded-lg cursor-pointer"
                  >
                    <LuInfo className="size-4 shrink-0 text-muted-foreground" />
                    <span>
                      {t("profiles.actions.details", {
                        defaultValue: "Chi tiết & Vân tay",
                      })}
                    </span>
                  </DropdownMenuItem>

                  <DropdownMenuSeparator className="my-1 bg-border/50" />

                  {/* Delete / Move to Trash */}
                  <DropdownMenuItem
                    onClick={() => {
                      setProfileToDelete(profile);
                    }}
                    className="gap-2.5 px-2.5 py-1.5 rounded-lg text-destructive focus:text-destructive focus:bg-destructive/10 cursor-pointer"
                  >
                    <LuTrash2 className="size-4 shrink-0" />
                    <span>
                      {t("profiles.actions.delete", {
                        defaultValue: "Chuyển vào Thùng rác",
                      })}
                    </span>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          );
        },
      },
    ],
    [sorting, t, setProfileForInfoDialog],
  );

  // Low-priority columns leave the table as the container narrows (most
  // expendable first); their data stays reachable via the profile info
  // dialog. Visibility (not CSS hiding) so table-fixed reclaims the width.
  const [columnVisibility, setColumnVisibility] =
    React.useState<VisibilityState>(() => ({
      created_at: false,
      ...(view.preferences.visibility ?? {}),
    }));

  React.useEffect(() => {
    if (view.loaded && view.preferences.visibility) {
      setColumnVisibility((prev) => ({
        ...prev,
        ...view.preferences.visibility,
        created_at: false,
      }));
    }
  }, [view.loaded, view.preferences.visibility]);

  const handleColumnVisibilityChange = React.useCallback(
    (updater: React.SetStateAction<VisibilityState>) => {
      setColumnVisibility((prev) => {
        const next = typeof updater === "function" ? updater(prev) : updater;
        view.setVisibility(next);
        return next;
      });
    },
    [view],
  );

  // Content columns grow proportionally with the container but never drop
  // below the compact-layout floor; the name column takes the remainder.
  // Computed in px from the observed container width because fixed table
  // layout ignores max()/calc() column widths.
  const [containerWidth, setContainerWidth] = React.useState(0);
  const [columnSizing, setColumnSizing] = React.useState<
    Record<string, number>
  >({});

  const table = useReactTable({
    data: profiles,
    columns,
    state: {
      sorting,
      rowSelection,
      columnVisibility,
      columnSizing,
    },
    onSortingChange: handleSortingChange,
    onRowSelectionChange: handleRowSelectionChange,
    onColumnVisibilityChange: handleColumnVisibilityChange,
    onColumnSizingChange: setColumnSizing,
    columnResizeMode: "onChange",
    enableRowSelection: (row) => {
      const profile = row.original;
      const isRunning =
        browserState.isClient && runningProfiles.has(profile.id);
      const isLaunching = launchingProfiles.has(profile.id);
      const isStopping = stoppingProfiles.has(profile.id);
      return !isRunning && !isLaunching && !isStopping;
    },
    getSortedRowModel: getSortedRowModel(),
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => row.id,
    meta: tableMeta,
  });

  const scrollParentRef = React.useRef<HTMLDivElement | null>(null);
  const columnWidth = React.useCallback(
    (id: string, sizePx: number) => {
      if (columnSizing[id] !== undefined) return `${sizePx}px`;
      const proportions: Record<string, { pct: number; floor: number }> = {
        tags: { pct: 0.12, floor: 100 },
        note: { pct: 0.1, floor: 80 },
        proxy: { pct: 0.13, floor: 110 },
        ext: { pct: 0.11, floor: 95 },
        dns: { pct: 0.11, floor: 95 },
      };
      const p = proportions[id];
      if (!p) return `${sizePx}px`;
      return `${Math.max(p.floor, Math.round(containerWidth * p.pct))}px`;
    },
    [columnSizing, containerWidth],
  );
  const sortedRows = table.getRowModel().rows;
  useScrollFade(scrollParentRef);

  React.useEffect(() => {
    const el = scrollParentRef.current;
    if (!el) return;
    const update = () => {
      const w = el.clientWidth;
      setContainerWidth(Math.round(w / 8) * 8);
      setColumnVisibility((prev: VisibilityState) => {
        const responsiveDefaults: VisibilityState = {
          created_at: false,
          dns: w >= 768,
          ext: w >= 672,
          note: w >= 576,
          tags: w >= 512,
        };
        const next: VisibilityState = {
          ...responsiveDefaults,
          ...(view.preferences.visibility ?? {}),
          created_at: false,
        };
        return Object.keys(next).every((k) => prev[k] === next[k])
          ? prev
          : next;
      });
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => {
      ro.disconnect();
    };
  }, [view.preferences.visibility]);

  const workspaceRef = React.useRef<HTMLDivElement | null>(null);
  const [inspectorLayout, setInspectorLayout] = React.useState(false);
  React.useEffect(() => {
    const element = workspaceRef.current;
    if (!element) return;
    const update = () => setInspectorLayout(element.clientWidth >= 1000);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Responsive row height based on user view density preference (compact: 36px, comfortable: 44px)
  const ROW_HEIGHT = view.preferences.density === "comfortable" ? 44 : 36;

  const rowVirtualizer = useVirtualizer({
    count: sortedRows.length,
    getScrollElement: () => scrollParentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  });

  const virtualRows = rowVirtualizer.getVirtualItems();
  const totalSize = rowVirtualizer.getTotalSize();
  const paddingTop = virtualRows.length > 0 ? virtualRows[0].start : 0;
  const paddingBottom =
    virtualRows.length > 0
      ? totalSize - virtualRows[virtualRows.length - 1].end
      : 0;

  const minimumWidth =
    Object.keys(view.preferences.sizing).length ||
    Object.values(view.preferences.visibility).some(Boolean)
      ? table
          .getVisibleLeafColumns()
          .reduce(
            (total, column) =>
              total +
              (column.id === "name" && !view.preferences.sizing.name
                ? 220
                : parseFloat(columnWidth(column.id, column.getSize()))),
            0,
          )
      : undefined;

  const columnLabels = React.useMemo(
    () => [
      { id: "name", label: t("common.labels.name"), hideable: false },
      { id: "created_at", label: t("search.fields.created"), hideable: false },
      { id: "tags", label: t("profileTable.tagsHeader") },
      { id: "note", label: t("profileTable.noteHeader") },
      { id: "proxy", label: t("profiles.table.proxy") },
      { id: "ext", label: t("profiles.table.ext") },
      { id: "dns", label: t("profiles.table.dns") },
    ],
    [t],
  );

  const allProfilesCount = totalProfilesCount ?? profiles.length;

  return (
    <>
      {/* Row 1: Group filter tabs + [+ New Profile] button */}
      <div className="mb-2 flex shrink-0 items-center justify-between gap-3">
        <div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto py-0.5 no-scrollbar">
          <Button
            size="sm"
            variant={
              !selectedGroupId || selectedGroupId === "__all__"
                ? "secondary"
                : "ghost"
            }
            className={cn(
              "h-7 shrink-0 gap-1.5 rounded-md px-2.5 text-xs font-medium transition-colors",
              !selectedGroupId || selectedGroupId === "__all__"
                ? "bg-secondary text-secondary-foreground shadow-xs"
                : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
            )}
            onClick={() => onGroupSelect?.("__all__")}
          >
            <span>{t("rail.profiles")}</span>
            <span className="rounded-full bg-foreground/10 px-1.5 py-0.2 text-[10px] tabular-nums font-semibold">
              {allProfilesCount}
            </span>
          </Button>

          {groups.map((group) => {
            const isSelected = selectedGroupId === group.id;
            return (
              <Button
                key={group.id}
                size="sm"
                variant={isSelected ? "secondary" : "ghost"}
                className={cn(
                  "h-7 shrink-0 gap-1.5 rounded-md px-2.5 text-xs font-medium transition-colors",
                  isSelected
                    ? "bg-secondary text-secondary-foreground shadow-xs"
                    : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
                )}
                onClick={() => onGroupSelect?.(group.id)}
              >
                <span className="max-w-[120px] truncate">{group.name}</span>
                {typeof group.profile_count === "number" && (
                  <span className="rounded-full bg-foreground/10 px-1.5 py-0.2 text-[10px] tabular-nums font-semibold">
                    {group.profile_count}
                  </span>
                )}
              </Button>
            );
          })}

          {onOpenGroupManagement && (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="size-7 shrink-0 text-muted-foreground hover:bg-muted/60 hover:text-foreground"
              title={t("groups.createTitle", { defaultValue: "Tạo nhóm mới" })}
              onClick={onOpenGroupManagement}
            >
              <LuFolderPlus className="size-3.5" />
            </Button>
          )}

          {(workspaceName ||
            organizationName ||
            (typeof maxProfiles === "number" && maxProfiles > 0)) && (
            <div className="hidden lg:flex items-center gap-2 pl-2 text-[11px] text-muted-foreground/80 font-mono select-none">
              <span className="text-muted-foreground/30">•</span>
              {organizationName && (
                <span className="font-sans font-medium text-foreground/80">
                  {organizationName}
                </span>
              )}
              {workspaceRole && (
                <span className="rounded bg-primary/10 px-1 py-0.2 text-[9px] font-semibold uppercase text-primary tracking-wider">
                  {workspaceRole}
                </span>
              )}
              {typeof maxProfiles === "number" && maxProfiles > 0 && (
                <>
                  <span className="text-muted-foreground/30">•</span>
                  <span>
                    {totalProfilesCount ?? profiles.length}/{maxProfiles} (
                    {Math.round(
                      ((totalProfilesCount ?? profiles.length) / maxProfiles) *
                        100,
                    )}
                    %)
                  </span>
                </>
              )}
            </div>
          )}
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          {onRefreshProfiles && (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              aria-label={t("profiles.table.refresh")}
              title={t("profiles.table.refresh")}
              disabled={isRefreshingProfiles}
              onClick={() => void onRefreshProfiles()}
              className="size-7"
            >
              <LuRefreshCw
                className={cn(
                  "size-3.5",
                  isRefreshingProfiles && "animate-spin",
                )}
              />
            </Button>
          )}
          {onCreateProfile && (
            <DropdownMenu>
              <div className="flex items-center rounded-md bg-primary text-primary-foreground shadow-xs">
                <Button
                  size="sm"
                  onClick={onCreateProfile}
                  className="h-7 gap-1.5 rounded-l-md rounded-r-none border-r border-primary-foreground/20 bg-transparent px-2.5 text-xs font-medium text-primary-foreground shadow-none hover:bg-white/10"
                  title={t("createProfile.title", {
                    defaultValue: "Tạo hồ sơ mới (Ctrl+N)",
                  })}
                >
                  <LuPlus className="size-3.5" />
                  <span>{t("header.newProfile", { defaultValue: "Mới" })}</span>
                </Button>
                <DropdownMenuTrigger asChild>
                  <Button
                    size="icon"
                    className="h-7 w-6 rounded-l-none rounded-r-md bg-transparent px-0 text-primary-foreground shadow-none hover:bg-white/10"
                    title={t("actions.addOptions", {
                      defaultValue: "Thêm mục khác...",
                    })}
                    aria-label={t("actions.addOptions", {
                      defaultValue: "Thao tác tạo nhanh",
                    })}
                  >
                    <LuChevronDown className="size-3" />
                  </Button>
                </DropdownMenuTrigger>
              </div>

              <DropdownMenuContent align="end" className="w-56">
                <DropdownMenuLabel className="text-xs font-semibold text-muted-foreground">
                  {t("actions.addOptions", {
                    defaultValue: "Thao tác tạo nhanh",
                  })}
                </DropdownMenuLabel>
                <DropdownMenuLabel className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  {t("actions.resources", { defaultValue: "Tài nguyên" })}
                </DropdownMenuLabel>
                <DropdownMenuItem
                  onClick={onCreateProfile}
                  className="gap-2 cursor-pointer text-xs"
                >
                  <LuGlobe className="size-3.5 text-primary" />
                  <span className="flex-1">
                    {t("createProfile.title", {
                      defaultValue: "Tạo hồ sơ mới",
                    })}
                  </span>
                  <span className="text-[10px] font-mono text-muted-foreground">
                    Ctrl+N
                  </span>
                </DropdownMenuItem>
                {onOpenGroupManagement && (
                  <DropdownMenuItem
                    onClick={onOpenGroupManagement}
                    className="gap-2 cursor-pointer text-xs"
                  >
                    <LuFolderPlus className="size-3.5 text-primary" />
                    <span className="flex-1">
                      {t("groups.createTitle", {
                        defaultValue: "Tạo nhóm mới",
                      })}
                    </span>
                    <span className="text-[10px] font-mono text-muted-foreground">
                      Ctrl+G
                    </span>
                  </DropdownMenuItem>
                )}
                {onOpenProxyManagement && (
                  <DropdownMenuItem
                    onClick={onOpenProxyManagement}
                    className="gap-2 cursor-pointer text-xs"
                  >
                    <LuShieldCheck className="size-3.5 text-primary" />
                    <span>
                      {t("proxies.newProxy", { defaultValue: "Thêm Proxy" })}
                    </span>
                  </DropdownMenuItem>
                )}
                {onOpenMemberManagement && (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuLabel className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      {t("actions.members", {
                        defaultValue: "Nhân sự & Thành viên",
                      })}
                    </DropdownMenuLabel>
                    <DropdownMenuItem
                      onClick={onOpenMemberManagement}
                      className="gap-2 cursor-pointer text-xs"
                    >
                      <LuUserPlus className="size-3.5 text-primary" />
                      <span>
                        {t("members.inviteMember", {
                          defaultValue: "Mời thành viên",
                        })}
                      </span>
                    </DropdownMenuItem>
                  </>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </div>

      {/* Row 2: Search, Filters, Active chips, Sort, and Columns */}
      {(filterControls || view.loaded) && (
        <div className="mb-2 flex shrink-0 flex-wrap items-center justify-between gap-2 rounded-lg border border-border/50 bg-card/50 p-1.5 backdrop-blur-xs">
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
            {filterControls}
          </div>
          <TableViewControls table={table} view={view} labels={columnLabels} />
        </div>
      )}
      <div
        ref={workspaceRef}
        data-slot="profile-workspace"
        className="relative flex min-h-0 min-w-0 flex-1 gap-3"
      >
        <div
          ref={scrollParentRef}
          className={cn(
            "scroll-fade relative min-h-0 flex-1 overflow-auto",
            // Clearance for the floating selection action bar (bottom-6 +
            // ~46px tall) so the last rows can scroll out from behind it.
            // Same predicate DataTableActionBar uses for its visibility.
            table.getFilteredSelectedRowModel().rows.length > 0 && "pb-20",
          )}
          style={
            {
              // Sticky table header is 32px tall (h-8); shift the top
              // fade band below it so the header stays fully opaque and
              // only body rows fade as they scroll past.
              "--scroll-fade-top-offset": "32px",
            } as React.CSSProperties
          }
        >
          <Table
            className={DATA_TABLE_CLASSES.table}
            style={{ minWidth: minimumWidth, width: minimumWidth }}
            containerClassName="overflow-visible"
          >
            <TableHeader
              className={cn(DATA_TABLE_CLASSES.header, "overflow-visible")}
            >
              {table.getHeaderGroups().map((headerGroup) => (
                <TableRow
                  key={headerGroup.id}
                  className={cn(
                    DATA_TABLE_CLASSES.headerRow,
                    "overflow-visible",
                  )}
                >
                  {headerGroup.headers.map((header) => {
                    return (
                      <TableHead
                        key={header.id}
                        role="columnheader"
                        aria-sort={
                          header.column.getIsSorted() === "asc"
                            ? "ascending"
                            : header.column.getIsSorted() === "desc"
                              ? "descending"
                              : undefined
                        }
                        className={DATA_TABLE_CLASSES.head}
                        style={{
                          width:
                            (
                              header.column.columnDef.meta as
                                | { flexWidth?: boolean }
                                | undefined
                            )?.flexWidth &&
                            columnSizing[header.column.id] === undefined
                              ? undefined
                              : columnWidth(
                                  header.column.id,
                                  header.column.getSize(),
                                ),
                        }}
                      >
                        {header.isPlaceholder
                          ? null
                          : flexRender(
                              header.column.columnDef.header,
                              header.getContext(),
                            )}
                        <TableColumnResizer
                          header={header}
                          label={header.column.id}
                        />
                      </TableHead>
                    );
                  })}
                </TableRow>
              ))}
            </TableHeader>
            <TableBody className="overflow-visible">
              {sortedRows.length === 0 ? (
                isLoading ? (
                  Array.from({ length: 8 }, (_, i) => (
                    <TableRow
                      key={`skeleton-${i}`}
                      className="border-0!"
                      style={{ height: `${ROW_HEIGHT}px` }}
                    >
                      <TableCell
                        colSpan={table.getVisibleLeafColumns().length}
                        className="py-0"
                      >
                        <div className="flex items-center gap-3">
                          <Skeleton className="size-7 shrink-0 rounded-md" />
                          <Skeleton
                            className="h-3"
                            style={{ width: `${30 + ((i * 17) % 40)}%` }}
                          />
                          <div className="flex-1" />
                          <Skeleton className="h-3 w-16" />
                          <Skeleton className="h-3 w-10" />
                        </div>
                      </TableCell>
                    </TableRow>
                  ))
                ) : showOnboardingEmptyState ? (
                  <TableRow className="border-0! hover:bg-transparent">
                    <TableCell
                      colSpan={table.getVisibleLeafColumns().length}
                      className="py-16"
                    >
                      <div className="flex flex-col items-center gap-3 text-center">
                        <Logo className="size-12 text-muted-foreground" />
                        <div>
                          <p className="text-sm font-medium text-foreground">
                            {t("profiles.table.emptyTitle")}
                          </p>
                          <p className="mt-1 text-xs text-muted-foreground">
                            {t("profiles.table.emptyHint")}
                          </p>
                        </div>
                        <div className="mt-1 flex gap-2">
                          {onCreateProfile && (
                            <RippleButton
                              size="sm"
                              disabled={!workspaceSelected}
                              onClick={onCreateProfile}
                            >
                              {t("profiles.table.emptyCreate")}
                            </RippleButton>
                          )}
                        </div>
                      </div>
                    </TableCell>
                  </TableRow>
                ) : (
                  <TableRow className="border-0! hover:bg-transparent">
                    <TableCell
                      colSpan={table.getVisibleLeafColumns().length}
                      className="py-16"
                    >
                      <div className="flex flex-col items-center gap-3 text-center">
                        <div className="grid size-12 place-items-center rounded-full bg-muted/60">
                          <LuUserSearch className="size-6 text-muted-foreground" />
                        </div>
                        <div>
                          <p className="text-sm font-medium text-foreground">
                            {t("profiles.table.emptyFilteredTitle")}
                          </p>
                          <p className="mt-1 text-xs text-muted-foreground">
                            {t("profiles.table.emptyFilteredHint")}
                          </p>
                        </div>
                        {onCreateProfile && (
                          <RippleButton
                            size="sm"
                            variant="outline"
                            className="mt-1"
                            disabled={!workspaceSelected}
                            onClick={onCreateProfile}
                          >
                            {t("profiles.table.emptyCreate")}
                          </RippleButton>
                        )}
                      </div>
                    </TableCell>
                  </TableRow>
                )
              ) : (
                <>
                  {paddingTop > 0 && (
                    <tr style={{ height: `${paddingTop}px` }}>
                      <td colSpan={table.getVisibleLeafColumns().length} />
                    </tr>
                  )}
                  {virtualRows.map((virtualRow) => {
                    const row = sortedRows[virtualRow.index];
                    const rowIsCrossOs = isCrossOsProfile(row.original);
                    const crossOsTitle = rowIsCrossOs
                      ? t("crossOs.viewOnly", {
                          os: getOSDisplayName(
                            row.original.host_os ||
                              row.original.wayfern_config?.os ||
                              "",
                          ),
                        })
                      : undefined;
                    return (
                      <TableRow
                        key={row.id}
                        data-table-row={row.id}
                        data-profile-id={row.id}
                        data-inspected={
                          profileForInfoDialog?.id === row.id
                            ? "true"
                            : undefined
                        }
                        data-moving={
                          profileDrag?.movingIds.has(row.id)
                            ? "true"
                            : undefined
                        }
                        data-state={row.getIsSelected() && "selected"}
                        title={crossOsTitle}
                        style={{ height: `${ROW_HEIGHT}px` }}
                        className={cn(
                          DATA_TABLE_CLASSES.row,
                          "overflow-visible",
                          rowIsCrossOs && "opacity-60",
                        )}
                      >
                        {row.getVisibleCells().map((cell) => (
                          <TableCell
                            key={cell.id}
                            className={cn(
                              DATA_TABLE_CLASSES.cell,
                              "overflow-visible",
                            )}
                            style={{
                              width: (
                                cell.column.columnDef.meta as
                                  | { flexWidth?: boolean }
                                  | undefined
                              )?.flexWidth
                                ? undefined
                                : columnWidth(
                                    cell.column.id,
                                    cell.column.getSize(),
                                  ),
                            }}
                          >
                            {flexRender(
                              cell.column.columnDef.cell,
                              cell.getContext(),
                            )}
                          </TableCell>
                        ))}
                      </TableRow>
                    );
                  })}
                  {paddingBottom > 0 && (
                    <tr style={{ height: `${paddingBottom}px` }}>
                      <td colSpan={table.getVisibleLeafColumns().length} />
                    </tr>
                  )}
                </>
              )}
            </TableBody>
          </Table>
        </div>

        {profileForInfoDialog &&
          (() => {
            const infoProfile =
              profiles.find((p) => p.id === profileForInfoDialog.id) ??
              profileForInfoDialog;
            const infoIsRunning =
              browserState.isClient && runningProfiles.has(infoProfile.id);
            const infoIsLaunching = launchingProfiles.has(infoProfile.id);
            const infoIsStopping = stoppingProfiles.has(infoProfile.id);
            const infoIsCrossOs = isCrossOsProfile(infoProfile);
            const infoIsDisabled =
              infoIsRunning ||
              infoIsLaunching ||
              infoIsStopping ||
              infoIsCrossOs;
            return (
              <ProfileInfoDialog
                presentation={inspectorLayout ? "inspector" : "modal"}
                isOpen={profileForInfoDialog !== null}
                workspaceId={workspaceId}
                onClose={() => {
                  setProfileForInfoDialog(null);
                }}
                profile={infoProfile}
                storedProxies={storedProxies}
                onOpenTrafficDialog={(profileId) => {
                  const profile = profiles.find((p) => p.id === profileId);
                  setTrafficDialogProfile({
                    id: profileId,
                    name: profile?.name,
                  });
                }}
                onAssignProfilesToGroup={onAssignProfilesToGroup}
                onConfigureWayfern={onConfigureWayfern}
                onCopyCookiesToProfile={onCopyCookiesToProfile}
                onOpenCookieManagement={onOpenCookieManagement}
                onAssignExtensionGroup={onAssignExtensionGroup}
                onOpenBypassRules={(profile) => {
                  setBypassRulesProfile(profile);
                }}
                onOpenDnsBlocklist={(profile) => {
                  setDnsBlocklistProfile(profile);
                }}
                onOpenLaunchHook={(profile) => {
                  setLaunchHookProfile(profile);
                }}
                onCloneProfile={onCloneProfile}
                onLaunchWithSync={onLaunchWithSync}
                onSetPassword={onSetPassword}
                onChangePassword={onChangePassword}
                onRemovePassword={onRemovePassword}
                onDeleteProfile={(profile) => {
                  setProfileForInfoDialog(null);
                  setProfileToDelete(profile);
                }}
                crossOsUnlocked={crossOsUnlocked}
                isRunning={infoIsRunning}
                isDisabled={infoIsDisabled}
                isCrossOs={infoIsCrossOs}
                syncStatuses={syncStatuses}
              />
            );
          })()}
      </div>
      <DeleteConfirmationDialog
        isOpen={profileToDelete !== null}
        onClose={() => {
          setProfileToDelete(null);
        }}
        onConfirm={handleDelete}
        title={t("profiles.delete.title")}
        description={t("profiles.delete.description", {
          profileName: profileToDelete?.name ?? "",
        })}
        confirmButtonText={t("profiles.delete.confirmButton")}
        isLoading={isDeleting}
      />
      <DataTableActionBar table={table}>
        <DataTableActionBarSelection table={table} />
        {onBulkRun && (
          <span className="relative inline-flex">
            <DataTableActionBarAction
              tooltip={
                bulkActionsUnlocked
                  ? t("profiles.actionBar.runSelected")
                  : t("profiles.actionBar.proRequired")
              }
              onClick={bulkActionsUnlocked ? onBulkRun : undefined}
              disabled={!bulkActionsUnlocked}
              size="icon"
            >
              <LuPlay className="fill-current" />
            </DataTableActionBarAction>
            {!bulkActionsUnlocked && (
              <ProBadge className="pointer-events-none absolute -top-2 -right-2" />
            )}
          </span>
        )}
        {onBulkStop && (
          <span className="relative inline-flex">
            <DataTableActionBarAction
              tooltip={
                bulkActionsUnlocked
                  ? t("profiles.actionBar.stopSelected")
                  : t("profiles.actionBar.proRequired")
              }
              onClick={bulkActionsUnlocked ? onBulkStop : undefined}
              disabled={!bulkActionsUnlocked}
              size="icon"
            >
              <LuSquare className="fill-current" />
            </DataTableActionBarAction>
            {!bulkActionsUnlocked && (
              <ProBadge className="pointer-events-none absolute -top-2 -right-2" />
            )}
          </span>
        )}
        {onBulkGroupAssignment && (
          <DataTableActionBarAction
            tooltip={t("profiles.actionBar.assignToGroup")}
            onClick={onBulkGroupAssignment}
            size="icon"
          >
            <LuUsers />
          </DataTableActionBarAction>
        )}
        {onBulkProxyAssignment && (
          <DataTableActionBarAction
            tooltip={t("profiles.actionBar.assignProxy")}
            onClick={onBulkProxyAssignment}
            size="icon"
          >
            <FiWifi />
          </DataTableActionBarAction>
        )}
        {onBulkExtensionGroupAssignment && (
          <DataTableActionBarAction
            tooltip={t("profiles.actionBar.assignExtensionGroup")}
            onClick={onBulkExtensionGroupAssignment}
            size="icon"
          >
            <LuPuzzle />
          </DataTableActionBarAction>
        )}
        {onBulkCopyCookies && (
          <DataTableActionBarAction
            tooltip={t("profiles.actionBar.copyCookies")}
            onClick={onBulkCopyCookies}
            size="icon"
          >
            <LuCookie />
          </DataTableActionBarAction>
        )}
        {onBulkShare && (
          <DataTableActionBarAction
            tooltip={t("profileSharing.shareSelected")}
            onClick={onBulkShare}
          >
            <LuUsers />
            <span>{t("profileSharing.share")}</span>
          </DataTableActionBarAction>
        )}
        {onBulkDelete && (
          <DataTableActionBarAction
            tooltip={t("common.buttons.delete")}
            onClick={onBulkDelete}
            size="icon"
            variant="destructive"
            className="border-destructive bg-destructive/50 hover:bg-destructive/70"
          >
            <LuTrash2 />
          </DataTableActionBarAction>
        )}
      </DataTableActionBar>
      {trafficDialogProfile && (
        <TrafficDetailsDialog
          isOpen={trafficDialogProfile !== null}
          onClose={() => {
            setTrafficDialogProfile(null);
          }}
          profileId={trafficDialogProfile.id}
          profileName={trafficDialogProfile.name}
        />
      )}
      <ProfileBypassRulesDialog
        isOpen={bypassRulesProfile !== null}
        onClose={() => {
          setBypassRulesProfile(null);
        }}
        profileId={bypassRulesProfile?.id ?? null}
        initialRules={bypassRulesProfile?.proxy_bypass_rules ?? []}
      />
      <ProfileDnsBlocklistDialog
        isOpen={dnsBlocklistProfile !== null}
        onClose={() => {
          setDnsBlocklistProfile(null);
        }}
        profileId={dnsBlocklistProfile?.id ?? null}
        currentLevel={dnsBlocklistProfile?.dns_blocklist ?? null}
      />
      <ProfileLaunchHookDialog
        isOpen={launchHookProfile !== null}
        onClose={() => {
          setLaunchHookProfile(null);
        }}
        profileId={launchHookProfile?.id ?? null}
        currentLaunchHook={launchHookProfile?.launch_hook ?? null}
      />
    </>
  );
}
