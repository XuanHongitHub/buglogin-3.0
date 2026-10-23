"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCheck,
  LuInfo,
  LuKey,
  LuPlay,
  LuSearch,
  LuShield,
  LuShieldCheck,
  LuUsers,
} from "react-icons/lu";
import { LoadingButton } from "@/components/loading-button";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { resolveHumanIdentity } from "@/lib/identity-display";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type {
  GroupShareGrant,
  Member,
  MemberRole,
} from "../../frontend/src/generated/command-registry";
import { RippleButton } from "./ui/ripple";

interface GroupSharingModalProps {
  isOpen: boolean;
  onClose: () => void;
  groupId: string;
  groupName: string;
  profileCount?: number;
  workspaceId?: string;
}

type GroupAction = "execute" | "update" | "read" | "share";

interface MemberGrantState {
  member: Member;
  granted: boolean;
  action: GroupAction;
  existingGrantId?: string;
  revision: number;
}

const desktopApi = createTauriDesktopApi();

export function GroupSharingModal({
  isOpen,
  onClose,
  groupId,
  groupName,
  profileCount = 0,
  workspaceId: propWorkspaceId,
}: GroupSharingModalProps) {
  const { t } = useTranslation();
  const [workspaceId, setWorkspaceId] = useState(propWorkspaceId || "");
  const [isLoading, setIsLoading] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [grantStates, setGrantStates] = useState<
    Record<string, MemberGrantState>
  >({});

  // Resolve active workspace if not passed
  useEffect(() => {
    if (propWorkspaceId) {
      setWorkspaceId(propWorkspaceId);
      return;
    }
    let disposed = false;
    void invoke<Array<{ id: string; name: string; selected?: boolean }>>(
      "get_available_workspaces",
    )
      .then((workspaces) => {
        if (disposed) return;
        const active = workspaces.find((w) => w.selected) || workspaces[0];
        if (active) setWorkspaceId(active.id);
      })
      .catch((err) =>
        console.error("Failed to load workspace for sharing:", err),
      );
    return () => {
      disposed = true;
    };
  }, [propWorkspaceId]);

  // Load members and existing grants for this group
  const loadData = useCallback(async () => {
    if (!workspaceId || !groupId) return;
    setIsLoading(true);
    try {
      const [membersPage, shareGrants] = await Promise.all([
        desktopApi.listMembers(workspaceId).catch((err) => {
          console.warn("Could not list members:", err);
          return { items: [] as Member[] };
        }),
        desktopApi.listGroupShares({ workspaceId, groupId }).catch((err) => {
          console.warn("Could not list group shares:", err);
          return [] as GroupShareGrant[];
        }),
      ]);

      const grantMap = new Map<string, GroupShareGrant>();
      for (const grant of shareGrants) {
        if (!grant.revokedAt) {
          grantMap.set(grant.memberId, grant);
        }
      }

      const nextStates: Record<string, MemberGrantState> = {};
      for (const member of membersPage.items) {
        const existing = grantMap.get(member.id);
        nextStates[member.id] = {
          member,
          granted:
            Boolean(existing) ||
            member.role === "owner" ||
            member.role === "admin",
          action: (existing?.action as GroupAction) || "execute",
          existingGrantId: existing?.id,
          revision: existing?.revision || 0,
        };
      }
      setGrantStates(nextStates);
    } catch (error) {
      console.error("Failed to load group sharing data:", error);
      showErrorToast(
        t("groups.loadSharingFailed", {
          defaultValue: "Không thể tải phân quyền nhóm",
        }),
      );
    } finally {
      setIsLoading(false);
    }
  }, [workspaceId, groupId, t]);

  useEffect(() => {
    if (isOpen) {
      void loadData();
    }
  }, [isOpen, loadData]);

  const handleToggleMember = (memberId: string, currentGranted: boolean) => {
    setGrantStates((prev) => {
      const cur = prev[memberId];
      if (!cur) return prev;
      return {
        ...prev,
        [memberId]: {
          ...cur,
          granted: !currentGranted,
        },
      };
    });
  };

  const handleActionChange = (memberId: string, action: GroupAction) => {
    setGrantStates((prev) => {
      const cur = prev[memberId];
      if (!cur) return prev;
      return {
        ...prev,
        [memberId]: {
          ...cur,
          action,
        },
      };
    });
  };

  const handleSaveGrants = async () => {
    if (!workspaceId || !groupId) return;
    setIsSaving(true);
    try {
      const operations: Promise<unknown>[] = [];

      for (const state of Object.values(grantStates)) {
        // Skip owners/admins as they implicitly have all access
        if (state.member.role === "owner" || state.member.role === "admin") {
          continue;
        }

        if (state.granted && !state.existingGrantId) {
          // Grant new share
          operations.push(
            desktopApi.createGroupShare({
              workspaceId,
              groupId,
              memberId: state.member.id,
              action: state.action,
              effect: "allow",
            }),
          );
        } else if (!state.granted && state.existingGrantId) {
          // Revoke existing share
          operations.push(
            desktopApi.revokeGroupShare({
              workspaceId,
              groupId,
              grantId: state.existingGrantId,
              expectedRevision: state.revision,
            }),
          );
        }
      }

      await Promise.all(operations);
      showSuccessToast(
        t("groups.shareSaved", {
          defaultValue: "Đã lưu phân quyền nhóm thành công",
        }),
      );
      onClose();
    } catch (err) {
      console.error("Failed to save group share grants:", err);
      showErrorToast(
        t("groups.shareSaveFailed", {
          defaultValue: "Không thể lưu phân quyền nhóm. Vui lòng thử lại.",
        }),
      );
    } finally {
      setIsSaving(false);
    }
  };

  const filteredMembers = useMemo(() => {
    const list = Object.values(grantStates);
    if (!searchQuery.trim()) return list;
    const q = searchQuery.toLowerCase();
    return list.filter((item, idx) => {
      const resolved = resolveHumanIdentity(
        item.member.accountId,
        item.member.role,
        idx,
        t,
      );
      return (
        resolved.displayName.toLowerCase().includes(q) ||
        resolved.email.toLowerCase().includes(q) ||
        item.member.role.toLowerCase().includes(q) ||
        item.member.accountId.toLowerCase().includes(q)
      );
    });
  }, [grantStates, searchQuery, t]);

  const grantedCount = useMemo(() => {
    return Object.values(grantStates).filter((s) => s.granted).length;
  }, [grantStates]);

  const getRoleBadge = (role: MemberRole) => {
    switch (role) {
      case "owner":
        return (
          <Badge className="bg-amber-500/15 text-amber-600 border-amber-300 text-[10px] gap-1 font-semibold">
            <LuShieldCheck className="size-3" />
            <span>Owner</span>
          </Badge>
        );
      case "admin":
        return (
          <Badge className="bg-primary/15 text-primary border-primary/30 text-[10px] gap-1 font-semibold">
            <LuShield className="size-3" />
            <span>Admin</span>
          </Badge>
        );
      case "operator":
        return (
          <Badge
            variant="outline"
            className="bg-muted text-foreground text-[10px] gap-1 font-medium"
          >
            <LuPlay className="size-2.5 text-muted-foreground" />
            <span>Operator</span>
          </Badge>
        );
      default:
        return (
          <Badge
            variant="outline"
            className="text-muted-foreground text-[10px]"
          >
            <span>Member</span>
          </Badge>
        );
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="max-w-2xl p-6">
        <DialogHeader className="border-b border-border/50 pb-3">
          <div className="flex items-center gap-3">
            <div className="grid size-10 place-items-center rounded-xl bg-primary/10 text-primary shadow-xs">
              <LuUsers className="size-5" />
            </div>
            <div>
              <DialogTitle className="text-base font-semibold flex items-center gap-2">
                <span>
                  {t("groups.shareTitle", {
                    defaultValue: "Phân Quyền Truy Cập Nhóm",
                  })}
                  :
                </span>
                <span className="text-primary font-bold">"{groupName}"</span>
              </DialogTitle>
              <DialogDescription className="text-xs text-muted-foreground mt-0.5">
                {t("groups.shareSubtitle", {
                  defaultValue:
                    "Nhóm hiện có {{count}} Profiles. Thành viên được cấp quyền sẽ tự động có quyền mở/chạy toàn bộ Profile thuộc nhóm này.",
                  count: profileCount,
                })}
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <div className="space-y-3.5 py-1 text-xs">
          {/* Search box */}
          <div className="relative">
            <LuSearch className="absolute left-3 top-2.5 size-4 text-muted-foreground" />
            <Input
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder={t("groups.searchMemberPlaceholder", {
                defaultValue:
                  "Tìm kiếm nhân sự theo tên, email hoặc User ID...",
              })}
              className="pl-9 h-9 text-xs"
            />
          </div>

          {/* Members Table */}
          <div className="rounded-lg border border-border overflow-hidden">
            <div className="max-h-[300px] overflow-y-auto">
              <table className="w-full text-left text-xs">
                <thead className="bg-muted/50 border-b border-border sticky top-0 font-medium text-muted-foreground text-[11px]">
                  <tr>
                    <th className="py-2.5 px-3 w-10 text-center">#</th>
                    <th className="py-2.5 px-3">Thành viên Workspace</th>
                    <th className="py-2.5 px-3 w-32">Vai trò</th>
                    <th className="py-2.5 px-3 w-44">Quyền trên nhóm này</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border/60">
                  {isLoading ? (
                    <tr>
                      <td
                        colSpan={4}
                        className="py-8 text-center text-muted-foreground"
                      >
                        {t("common.buttons.loading", {
                          defaultValue: "Đang tải dữ liệu...",
                        })}
                      </td>
                    </tr>
                  ) : filteredMembers.length === 0 ? (
                    <tr>
                      <td
                        colSpan={4}
                        className="py-8 text-center text-muted-foreground"
                      >
                        {t("groups.noMembersFound", {
                          defaultValue: "Không tìm thấy thành viên phù hợp",
                        })}
                      </td>
                    </tr>
                  ) : (
                    filteredMembers.map((item, idx) => {
                      const isOwnerOrAdmin =
                        item.member.role === "owner" ||
                        item.member.role === "admin";
                      const isChecked = isOwnerOrAdmin || item.granted;
                      const resolved = resolveHumanIdentity(
                        item.member.accountId,
                        item.member.role,
                        idx,
                        t,
                      );

                      return (
                        <tr
                          key={item.member.id}
                          className={cn(
                            "transition-colors hover:bg-muted/30",
                            isChecked && "bg-primary/5",
                          )}
                        >
                          <td className="py-2.5 px-3 text-center">
                            <Checkbox
                              checked={isChecked}
                              disabled={isOwnerOrAdmin}
                              onCheckedChange={() =>
                                handleToggleMember(item.member.id, item.granted)
                              }
                              aria-label={`Toggle access for ${resolved.displayName}`}
                            />
                          </td>
                          <td className="py-2.5 px-3">
                            <div className="flex items-center gap-2.5">
                              <div className="flex size-7 items-center justify-center rounded-full bg-primary/10 font-bold text-[11px] text-primary shrink-0">
                                {resolved.initials}
                              </div>
                              <div className="min-w-0">
                                <div className="font-semibold text-foreground text-xs truncate">
                                  {resolved.displayName}
                                </div>
                                <div className="text-[10px] text-muted-foreground truncate">
                                  {resolved.email}
                                </div>
                              </div>
                            </div>
                          </td>
                          <td className="py-2.5 px-3">
                            {getRoleBadge(item.member.role)}
                          </td>
                          <td className="py-2.5 px-3">
                            {isOwnerOrAdmin ? (
                              <span className="text-[11px] text-muted-foreground italic flex items-center gap-1">
                                <LuShieldCheck className="size-3 text-primary" />
                                <span>Toàn quyền (Quản trị)</span>
                              </span>
                            ) : item.granted ? (
                              <Select
                                value={item.action}
                                onValueChange={(val) =>
                                  handleActionChange(
                                    item.member.id,
                                    val as GroupAction,
                                  )
                                }
                              >
                                <SelectTrigger className="h-7 text-[11px]">
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  <SelectItem
                                    value="execute"
                                    className="text-xs"
                                  >
                                    <span className="inline-flex items-center gap-1.5">
                                      <LuPlay className="size-3 text-emerald-500" />
                                      Chỉ Mở/Chạy Profile
                                    </span>
                                  </SelectItem>
                                  <SelectItem
                                    value="update"
                                    className="text-xs"
                                  >
                                    <span className="inline-flex items-center gap-1.5">
                                      <LuKey className="size-3 text-amber-500" />
                                      Mở & Chỉnh Sửa
                                    </span>
                                  </SelectItem>
                                  <SelectItem value="share" className="text-xs">
                                    <span className="inline-flex items-center gap-1.5">
                                      <LuUsers className="size-3 text-blue-500" />
                                      Toàn Quyền Nhóm
                                    </span>
                                  </SelectItem>
                                </SelectContent>
                              </Select>
                            ) : (
                              <span className="text-[11px] text-muted-foreground/60 italic">
                                -- Chưa cấp quyền --
                              </span>
                            )}
                          </td>
                        </tr>
                      );
                    })
                  )}
                </tbody>
              </table>
            </div>
          </div>

          {/* Dynamic Cascade Tip Callout */}
          <div className="rounded-lg border border-primary/20 bg-primary/5 p-3 flex items-start gap-2.5 text-xs text-foreground/85">
            <LuInfo className="size-4 text-primary shrink-0 mt-0.5" />
            <div className="space-y-0.5">
              <span className="font-semibold text-primary">
                Cơ chế phân quyền động (Dynamic Access Cascade):
              </span>
              <p className="text-[11px] text-muted-foreground leading-relaxed">
                Khi bạn chuyển bất kỳ Profile nào vào nhóm{" "}
                <strong>"{groupName}"</strong>, tất cả thành viên được cấp quyền
                ở trên sẽ tự động có quyền mở và vận hành Profile đó ngay lập
                tức mà không cần phân quyền lại từng profile riêng lẻ.
              </p>
            </div>
          </div>
        </div>

        <DialogFooter className="border-t border-border/50 pt-3 flex items-center justify-between sm:justify-between">
          <div className="text-xs text-muted-foreground">
            Đã cấp quyền:{" "}
            <strong className="text-foreground">{grantedCount}</strong> thành
            viên
          </div>
          <div className="flex items-center gap-2">
            <RippleButton
              variant="outline"
              size="sm"
              onClick={onClose}
              disabled={isSaving}
              className="h-8 text-xs font-medium cursor-pointer"
            >
              {t("common.buttons.cancel", { defaultValue: "Hủy" })}
            </RippleButton>
            <LoadingButton
              size="sm"
              isLoading={isSaving}
              onClick={() => void handleSaveGrants()}
              className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
            >
              <LuCheck className="size-3.5" />
              <span>
                {t("common.buttons.save", { defaultValue: "Lưu Phân Quyền" })}
              </span>
            </LoadingButton>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
