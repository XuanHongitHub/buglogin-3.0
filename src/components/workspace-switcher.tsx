"use client";

import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowUpDown,
  LuBuilding2,
  LuCheck,
  LuChevronDown,
  LuChevronLeft,
  LuChevronRight,
  LuFolderKanban,
  LuLayers,
  LuPlus,
  LuRefreshCw,
  LuSlidersHorizontal,
} from "react-icons/lu";
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
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";

export interface WorkspaceItem {
  id: string;
  name: string;
  organizationId?: string;
  organizationName?: string;
  type?: "personal" | "team";
  selected?: boolean;
  role?: string;
  profileCount?: number;
  maxProfiles?: number;
  memberCount?: number;
  maxMembers?: number;
}

interface WorkspaceSwitcherProps {
  workspaces: WorkspaceItem[];
  selectedWorkspaceId: string;
  onWorkspaceSelect: (workspaceId: string) => void;
  onManageWorkspaces?: () => void;
  onOpenOrgCenter?: () => void;
  onCreateWorkspace?: () => void;
  collapsed?: boolean;
  totalProfilesCount?: number;
  isLoading?: boolean;
  variant?: "rail" | "header";
}

export function WorkspaceSwitcher({
  workspaces = [],
  selectedWorkspaceId,
  onWorkspaceSelect,
  onManageWorkspaces,
  onOpenOrgCenter,
  onCreateWorkspace,
  collapsed = false,
  totalProfilesCount,
  isLoading = false,
  variant = "rail",
}: WorkspaceSwitcherProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [filterOrg, setFilterOrg] = useState<string>("all");
  const [sortBy, setSortBy] = useState<"active" | "name" | "profiles">(
    "active",
  );
  const [groupPages, setGroupPages] = useState<Record<string, number>>({});

  // Active workspace
  const activeWorkspace = useMemo(() => {
    if (!workspaces || workspaces.length === 0) return null;
    return (
      workspaces.find((w) => w.id === selectedWorkspaceId) || workspaces[0]
    );
  }, [workspaces, selectedWorkspaceId]);

  // Extract all available Org names for filter pills
  const availableOrgs = useMemo(() => {
    const set = new Set<string>();
    for (const ws of workspaces) {
      const orgName =
        ws.organizationName?.trim() ||
        (ws.type === "personal"
          ? t("header.personalSpace", { defaultValue: "Không Gian Cá Nhân" })
          : t("header.mainOrg", { defaultValue: "Tổ Chức Chính" }));
      set.add(orgName);
    }
    return Array.from(set);
  }, [workspaces, t]);

  // Group workspaces by Organization with sorting and filtering
  const orgGroups = useMemo(() => {
    const groups = new Map<
      string,
      {
        orgName: string;
        items: WorkspaceItem[];
        totalProfiles: number;
        maxProfiles: number;
        hasSelected: boolean;
      }
    >();

    for (const ws of workspaces) {
      const orgName =
        ws.organizationName?.trim() ||
        (ws.type === "personal"
          ? t("header.personalSpace", { defaultValue: "Không Gian Cá Nhân" })
          : t("header.mainOrg", { defaultValue: "Tổ Chức Chính" }));
      if (filterOrg !== "all" && orgName !== filterOrg) continue;

      const current = groups.get(orgName) || {
        orgName,
        items: [],
        totalProfiles: 0,
        maxProfiles: 0,
        hasSelected: false,
      };
      if (ws.id === selectedWorkspaceId) {
        current.hasSelected = true;
      }
      current.items.push(ws);
      current.totalProfiles +=
        ws.id === selectedWorkspaceId
          ? (totalProfilesCount ?? ws.profileCount ?? 0)
          : (ws.profileCount ?? 0);
      current.maxProfiles += ws.maxProfiles ?? 0;
      groups.set(orgName, current);
    }

    if (groups.size === 0 && filterOrg === "all" && activeWorkspace) {
      const fallbackOrg =
        activeWorkspace.organizationName ||
        t("header.mainOrg", { defaultValue: "Tổ Chức Chính" });
      groups.set(fallbackOrg, {
        orgName: fallbackOrg,
        items: [activeWorkspace],
        totalProfiles: totalProfilesCount ?? 0,
        maxProfiles: activeWorkspace.maxProfiles ?? 0,
        hasSelected: true,
      });
    }

    const groupList = Array.from(groups.values());

    // Sort items within each group according to sortBy
    for (const g of groupList) {
      g.items.sort((a, b) => {
        if (sortBy === "active") {
          if (a.id === selectedWorkspaceId) return -1;
          if (b.id === selectedWorkspaceId) return 1;
          return a.name.localeCompare(b.name);
        }
        if (sortBy === "name") {
          return a.name.localeCompare(b.name);
        }
        if (sortBy === "profiles") {
          const aCount = a.profileCount ?? 0;
          const bCount = b.profileCount ?? 0;
          return bCount - aCount;
        }
        return 0;
      });
    }

    // Sort groups: group containing active workspace first
    groupList.sort((a, b) => {
      if (a.hasSelected && !b.hasSelected) return -1;
      if (!a.hasSelected && b.hasSelected) return 1;
      return a.orgName.localeCompare(b.orgName);
    });

    return groupList;
  }, [
    workspaces,
    activeWorkspace,
    selectedWorkspaceId,
    totalProfilesCount,
    filterOrg,
    sortBy,
    t,
  ]);

  const activeProfCount =
    totalProfilesCount ?? activeWorkspace?.profileCount ?? 0;
  const activeMaxProf = activeWorkspace?.maxProfiles;

  const cycleSort = () => {
    setSortBy((current) => {
      if (current === "active") return "name";
      if (current === "name") return "profiles";
      return "active";
    });
  };

  function renderRoleBadge(role?: string) {
    if (!role) return null;
    const r = role.toLowerCase();
    if (r === "owner") {
      return (
        <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-amber-500/10 text-amber-500 border-amber-500/30 shrink-0">
          OWNER
        </span>
      );
    }
    if (r === "admin") {
      return (
        <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-blue-500/10 text-blue-400 border-blue-500/30 shrink-0">
          ADMIN
        </span>
      );
    }
    if (r === "manager" || r === "operator" || r === "lead") {
      return (
        <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-emerald-500/10 text-emerald-400 border-emerald-500/30 shrink-0">
          LEAD
        </span>
      );
    }
    return (
      <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-muted/80 text-muted-foreground/90 border-border/60 shrink-0">
        MEMBER
      </span>
    );
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          role="combobox"
          aria-expanded={open}
          aria-busy={isLoading}
          aria-label={t("header.workspaceLabel", { defaultValue: "Workspace" })}
          className={cn(
            "border-border/60 bg-background/80 font-medium transition-all hover:bg-muted/80 shadow-2xs",
            collapsed
              ? "size-8 p-0 justify-center rounded-md"
              : "w-full justify-between h-11 px-2.5 rounded-lg text-xs flex items-center gap-2",
          )}
        >
          {collapsed ? (
            <span className="grid size-5 place-items-center rounded bg-primary/10 text-primary">
              <LuBuilding2 className="size-3.5" />
            </span>
          ) : (
            <>
              <div className="flex min-w-0 items-center gap-2 flex-1 text-left">
                <span className="grid size-6 shrink-0 place-items-center rounded-md bg-primary/10 text-primary border border-primary/20">
                  <LuBuilding2 className="size-3.5" />
                </span>
                <div className="flex flex-col min-w-0 flex-1">
                  <span
                    className="truncate text-foreground font-semibold text-xs leading-tight w-full"
                    title={activeWorkspace?.name}
                  >
                    {activeWorkspace?.name ||
                      (isLoading
                        ? t("header.loadingWorkspaces", {
                            defaultValue: "Đang tải workspace…",
                          })
                        : t("header.selectWorkspace", {
                            defaultValue: "Chọn Workspace",
                          }))}
                  </span>
                  {activeWorkspace ? (
                    <div className="flex items-center gap-1.5 text-[10px] text-muted-foreground leading-tight mt-0.5 min-w-0">
                      {renderRoleBadge(activeWorkspace.role)}
                      <span
                        className="truncate font-medium text-foreground/70"
                        title={activeWorkspace.organizationName}
                      >
                        {activeWorkspace.organizationName ||
                          t("header.organization", { defaultValue: "Tổ chức" })}
                      </span>
                      <span className="shrink-0">•</span>
                      <span className="font-mono text-primary font-medium shrink-0">
                        {activeMaxProf != null
                          ? `${activeProfCount}/${activeMaxProf}`
                          : `${activeProfCount} hồ sơ`}
                      </span>
                    </div>
                  ) : null}
                </div>
              </div>
              {isLoading ? (
                <LuRefreshCw
                  className="ml-0.5 size-3.5 shrink-0 animate-spin text-muted-foreground"
                  aria-label={t("common.buttons.loading", {
                    defaultValue: "Đang tải",
                  })}
                />
              ) : (
                <LuChevronDown className="ml-0.5 size-3.5 shrink-0 text-muted-foreground opacity-70" />
              )}
            </>
          )}
        </Button>
      </PopoverTrigger>

      <PopoverContent
        className="w-96 p-0 shadow-xl border-border/80 flex flex-col overflow-hidden"
        align="start"
        sideOffset={6}
      >
        <Command className="flex flex-col flex-1 min-h-0">
          <CommandInput
            placeholder={t("workspaceSwitcher.searchPlaceholder", {
              defaultValue: "Tìm tổ chức hoặc không gian làm việc...",
            })}
            className="h-9 text-xs"
          />

          {/* Pinned Current Workspace Header - Always visible with zero scrolling */}
          {activeWorkspace && (
            <div className="bg-primary/5 border-b border-border/60 px-3 py-2 shrink-0">
              <div className="flex items-center justify-between text-[10px] font-semibold uppercase tracking-wider mb-1">
                <span className="flex items-center gap-1.5 text-primary">
                  <span className="size-1.5 rounded-full bg-primary animate-pulse" />
                  {t("workspaceSwitcher.currentWorkspace", {
                    defaultValue: "Workspace Hiện Tại",
                  })}
                </span>
                {renderRoleBadge(activeWorkspace.role)}
              </div>
              <div className="flex items-center justify-between gap-2">
                <div className="flex min-w-0 flex-1 items-center gap-2">
                  <span className="grid size-5 shrink-0 place-items-center rounded bg-primary/10 text-primary border border-primary/20">
                    <LuBuilding2 className="size-3" />
                  </span>
                  <div className="flex flex-col min-w-0 flex-1">
                    <span
                      className="truncate text-xs font-semibold text-foreground"
                      title={activeWorkspace.name}
                    >
                      {activeWorkspace.name}
                    </span>
                    <span
                      className="truncate text-[10px] text-muted-foreground"
                      title={activeWorkspace.organizationName}
                    >
                      {activeWorkspace.organizationName ||
                        t("header.organization", { defaultValue: "Tổ chức" })}
                    </span>
                  </div>
                </div>
                <div className="flex items-center gap-1.5 shrink-0">
                  <span className="text-[10px] font-mono font-medium text-foreground bg-background/80 px-1.5 py-0.5 rounded border border-border/50">
                    {activeMaxProf != null
                      ? `${activeProfCount}/${activeMaxProf}`
                      : `${activeProfCount} hồ sơ`}
                  </span>
                  <span className="text-[10px] font-medium text-primary bg-primary/10 px-1.5 py-0.5 rounded border border-primary/20 flex items-center gap-1">
                    <LuCheck className="size-3" />
                    {t("workspaceSwitcher.active", {
                      defaultValue: "Đang dùng",
                    })}
                  </span>
                </div>
              </div>
            </div>
          )}

          {/* Filters & Sorting Toolbar */}
          <div className="flex items-center justify-between gap-1.5 px-3 py-1.5 border-b border-border/40 bg-muted/20 shrink-0 text-[10px]">
            <div className="flex items-center gap-1 overflow-x-auto pb-0.5 min-w-0 flex-1 [&::-webkit-scrollbar]:hidden">
              <button
                type="button"
                onClick={() => setFilterOrg("all")}
                className={cn(
                  "px-2 py-0.5 rounded font-medium transition-colors cursor-pointer shrink-0",
                  filterOrg === "all"
                    ? "bg-primary text-primary-foreground font-semibold"
                    : "bg-muted/60 text-muted-foreground hover:bg-muted hover:text-foreground",
                )}
              >
                {t("workspaceSwitcher.allOrgs", { defaultValue: "Tất cả" })} (
                {workspaces.length})
              </button>
              {availableOrgs.map((org) => {
                const active = filterOrg === org;
                return (
                  <button
                    key={org}
                    type="button"
                    onClick={() => setFilterOrg(org)}
                    className={cn(
                      "px-2 py-0.5 rounded font-medium transition-colors cursor-pointer shrink-0 truncate max-w-[110px]",
                      active
                        ? "bg-primary text-primary-foreground font-semibold"
                        : "bg-muted/60 text-muted-foreground hover:bg-muted hover:text-foreground",
                    )}
                    title={org}
                  >
                    {org}
                  </button>
                );
              })}
            </div>

            <button
              type="button"
              onClick={cycleSort}
              className="flex items-center gap-1 px-1.5 py-0.5 rounded border border-border/60 bg-background/80 hover:bg-muted text-muted-foreground hover:text-foreground transition-colors shrink-0 font-medium cursor-pointer"
              title="Thay đổi thứ tự sắp xếp"
            >
              <LuArrowUpDown className="size-3 text-primary" />
              <span>
                {sortBy === "active"
                  ? "Đang dùng"
                  : sortBy === "name"
                    ? "Tên A-Z"
                    : "Profiles"}
              </span>
            </button>
          </div>

          <CommandList className="max-h-72 overflow-y-auto flex-1 min-h-0">
            <CommandEmpty className="flex items-center justify-center gap-2 py-3 text-center text-xs text-muted-foreground">
              {isLoading ? (
                <>
                  <LuRefreshCw className="size-3 animate-spin" />
                  {t("common.buttons.loading", {
                    defaultValue: "Đang tải dữ liệu...",
                  })}
                </>
              ) : (
                t("workspaceSwitcher.noWorkspaceFound", {
                  defaultValue: "Không tìm thấy không gian làm việc phù hợp.",
                })
              )}
            </CommandEmpty>

            {orgGroups.map((group) => {
              const currentPage = groupPages[group.orgName] || 0;
              const pageSize = 5;
              const totalPages = Math.ceil(group.items.length / pageSize);
              const pagedItems = group.items.slice(
                currentPage * pageSize,
                (currentPage + 1) * pageSize,
              );

              return (
                <div key={group.orgName} className="py-1">
                  <div className="px-3 py-1.5 flex items-center justify-between bg-muted/30 border-y border-border/40">
                    <div className="flex items-center gap-1.5 truncate flex-1 min-w-0 pr-2">
                      <LuFolderKanban className="size-3.5 text-primary shrink-0" />
                      <span
                        className="text-[11px] font-bold tracking-tight text-foreground uppercase truncate"
                        title={group.orgName}
                      >
                        {group.orgName}
                      </span>
                    </div>
                    <div className="text-[10px] text-muted-foreground font-mono flex items-center gap-1 shrink-0">
                      <span>
                        {t("workspaceSwitcher.quota", {
                          defaultValue: "Hạn mức:",
                        })}
                      </span>
                      <strong className="text-foreground">
                        {group.maxProfiles > 0
                          ? `${group.totalProfiles}/${group.maxProfiles}`
                          : `${group.totalProfiles} hồ sơ`}
                      </strong>
                    </div>
                  </div>

                  <CommandGroup>
                    {pagedItems.map((ws) => {
                      const isSelected = ws.id === selectedWorkspaceId;
                      const profCount = isSelected
                        ? (totalProfilesCount ?? ws.profileCount ?? 0)
                        : (ws.profileCount ?? 0);
                      const maxProf = ws.maxProfiles;
                      const pct =
                        maxProf && maxProf > 0
                          ? Math.min(
                              100,
                              Math.round((profCount / maxProf) * 100),
                            )
                          : null;

                      return (
                        <CommandItem
                          key={ws.id}
                          value={`${ws.name} ${group.orgName}`}
                          onSelect={() => {
                            onWorkspaceSelect(ws.id);
                            setOpen(false);
                          }}
                          className={cn(
                            "flex cursor-pointer items-center justify-between gap-2 px-3 py-2 text-xs transition-colors",
                            isSelected &&
                              "bg-primary/10 font-semibold text-primary",
                          )}
                        >
                          <div className="flex min-w-0 flex-1 items-start gap-2">
                            <span
                              className={cn(
                                "grid size-4 shrink-0 place-items-center rounded mt-0.5",
                                isSelected
                                  ? "text-primary"
                                  : "text-muted-foreground",
                              )}
                            >
                              <LuLayers className="size-3.5" />
                            </span>
                            <div className="flex flex-col min-w-0 flex-1">
                              <div className="flex items-center justify-between gap-1.5 min-w-0">
                                <span
                                  className="truncate text-foreground font-medium text-xs flex-1"
                                  title={ws.name}
                                >
                                  {ws.name}
                                </span>
                                {renderRoleBadge(ws.role)}
                                {isSelected && (
                                  <LuCheck className="size-3.5 text-primary shrink-0 ml-0.5" />
                                )}
                              </div>
                              <div className="flex items-center gap-2 mt-1">
                                {pct !== null && (
                                  <div className="h-1.5 w-16 rounded-full bg-muted overflow-hidden shrink-0">
                                    <div
                                      className="h-full bg-primary transition-all rounded-full"
                                      style={{ width: `${pct}%` }}
                                    />
                                  </div>
                                )}
                                <span className="text-[10px] text-muted-foreground font-mono">
                                  {maxProf != null
                                    ? `${profCount}/${maxProf} (${pct}%)`
                                    : `${profCount} hồ sơ`}
                                </span>
                              </div>
                            </div>
                          </div>
                        </CommandItem>
                      );
                    })}
                  </CommandGroup>

                  {totalPages > 1 && (
                    <div className="flex items-center justify-between px-3 py-1.5 border-t border-border/30 bg-muted/10 text-[10px] text-muted-foreground">
                      <span>
                        {currentPage * pageSize + 1}-
                        {Math.min(
                          (currentPage + 1) * pageSize,
                          group.items.length,
                        )}{" "}
                        / {group.items.length} ws
                      </span>
                      <div className="flex items-center gap-1.5">
                        <button
                          type="button"
                          disabled={currentPage === 0}
                          onClick={(e) => {
                            e.stopPropagation();
                            setGroupPages((prev) => ({
                              ...prev,
                              [group.orgName]: currentPage - 1,
                            }));
                          }}
                          className="size-5 rounded grid place-items-center bg-background border border-border/50 disabled:opacity-40 hover:bg-muted cursor-pointer"
                        >
                          <LuChevronLeft className="size-3" />
                        </button>
                        <span className="font-mono font-medium">
                          {currentPage + 1}/{totalPages}
                        </span>
                        <button
                          type="button"
                          disabled={currentPage >= totalPages - 1}
                          onClick={(e) => {
                            e.stopPropagation();
                            setGroupPages((prev) => ({
                              ...prev,
                              [group.orgName]: currentPage + 1,
                            }));
                          }}
                          className="size-5 rounded grid place-items-center bg-background border border-border/50 disabled:opacity-40 hover:bg-muted cursor-pointer"
                        >
                          <LuChevronRight className="size-3" />
                        </button>
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </CommandList>

          {/* Sticky Management Footer */}
          <div className="border-t border-border/60 bg-muted/30 p-1.5 flex flex-col gap-1 shrink-0">
            <button
              type="button"
              onClick={() => {
                setOpen(false);
                if (onManageWorkspaces) onManageWorkspaces();
                else if (onOpenOrgCenter) onOpenOrgCenter();
              }}
              className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-xs text-foreground font-medium hover:bg-muted/70 transition-colors w-full text-left cursor-pointer group"
            >
              <span className="grid size-5 place-items-center rounded bg-primary/10 text-primary group-hover:bg-primary group-hover:text-primary-foreground transition-colors shrink-0">
                <LuSlidersHorizontal className="size-3" />
              </span>
              <div className="flex flex-col min-w-0 flex-1">
                <span className="truncate leading-tight text-xs font-medium">
                  {t("workspaceSwitcher.orgCenterTitle", {
                    defaultValue: "Quản lý Tổ chức & Workspace",
                  })}
                </span>
                <span className="text-[10px] text-muted-foreground font-normal truncate leading-tight mt-0.5">
                  {t("workspaceSwitcher.orgCenterDesc", {
                    defaultValue: "Quản trị phân quyền, thành viên và quota",
                  })}
                </span>
              </div>
            </button>

            {onCreateWorkspace && (
              <button
                type="button"
                onClick={() => {
                  setOpen(false);
                  onCreateWorkspace();
                }}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-xs text-primary font-medium hover:bg-primary/10 transition-colors w-full text-left cursor-pointer"
              >
                <span className="grid size-5 place-items-center rounded bg-primary/10 text-primary shrink-0">
                  <LuPlus className="size-3" />
                </span>
                <span className="truncate">
                  {t("workspaceSwitcher.createNew", {
                    defaultValue: "Tạo không gian làm việc mới...",
                  })}
                </span>
              </button>
            )}
          </div>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
