"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowRightLeft,
  LuCheck,
  LuFolder,
  LuFolderPlus,
  LuInfo,
  LuSearch,
} from "react-icons/lu";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import { translateBackendError } from "@/lib/backend-errors";
import { cn } from "@/lib/utils";
import type { ProfileGroup } from "@/types";

interface GroupAssignmentDialogProps {
  isOpen: boolean;
  onClose: () => void;
  selectedProfiles: string[];
  onAssignmentComplete: () => void;
}

export function GroupAssignmentDialog({
  isOpen,
  onClose,
  selectedProfiles,
  onAssignmentComplete,
}: GroupAssignmentDialogProps) {
  const { t, i18n } = useTranslation();
  const isVi = i18n.language === "vi";

  const [groups, setGroups] = useState<ProfileGroup[]>([]);
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isAssigning, setIsAssigning] = useState(false);
  const [isCreatingGroup, setIsCreatingGroup] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showCreateGroup, setShowCreateGroup] = useState(false);
  const [newGroupName, setNewGroupName] = useState("");
  const [searchQuery, setSearchQuery] = useState("");

  const loadGroups = useCallback(async () => {
    setIsLoading(true);
    setError(null);
    try {
      const groupList = await invoke<ProfileGroup[]>("get_profile_groups");
      setGroups(groupList);
    } catch (err) {
      console.error("Failed to load groups:", err);
      setError(
        err instanceof Error ? err.message : t("groupManagement.loadFailed"),
      );
    } finally {
      setIsLoading(false);
    }
  }, [t]);

  const handleClose = useCallback(() => {
    if (isAssigning || isCreatingGroup) return;
    setShowCreateGroup(false);
    setNewGroupName("");
    onClose();
  }, [isAssigning, isCreatingGroup, onClose]);

  const handleCreateGroup = useCallback(async () => {
    const name = newGroupName.trim();
    if (!name || isCreatingGroup) return;

    setIsCreatingGroup(true);
    setError(null);
    try {
      const group = await invoke<ProfileGroup>("create_profile_group", {
        name,
      });
      setGroups((previous) => [...previous, group]);
      setSelectedGroupId(group.id);
      setNewGroupName("");
      setShowCreateGroup(false);
      toast.success(t("groups.createSuccess"));
    } catch (err) {
      const message = translateBackendError(t, err);
      setError(message);
      toast.error(message);
    } finally {
      setIsCreatingGroup(false);
    }
  }, [isCreatingGroup, newGroupName, t]);

  const handleAssign = useCallback(async () => {
    setIsAssigning(true);
    setError(null);
    try {
      await invoke("assign_profiles_to_group", {
        profileIds: selectedProfiles,
        groupId: selectedGroupId,
      });

      const groupName = selectedGroupId
        ? groups.find((g) => g.id === selectedGroupId)?.name ||
          t("groups.unknownGroup")
        : t("groups.noGroup", { defaultValue: "Chưa phân nhóm" });

      toast.success(
        isVi
          ? `Đã chuyển ${selectedProfiles.length} profile sang nhóm "${groupName}"`
          : `Assigned ${selectedProfiles.length} profile(s) to group "${groupName}"`,
      );
      onAssignmentComplete();
      onClose();
    } catch (err) {
      console.error("Failed to assign profiles to group:", err);
      const errorMessage =
        err instanceof Error
          ? err.message
          : t("groupAssignment.failedFallback", {
              defaultValue: "Không thể chuyển nhóm profile",
            });
      setError(errorMessage);
      toast.error(errorMessage);
    } finally {
      setIsAssigning(false);
    }
  }, [
    selectedProfiles,
    selectedGroupId,
    groups,
    onAssignmentComplete,
    onClose,
    isVi,
    t,
  ]);

  useEffect(() => {
    if (isOpen) {
      void loadGroups();
      setSelectedGroupId(null);
      setError(null);
      setSearchQuery("");
      setShowCreateGroup(false);
      setNewGroupName("");
    }
  }, [isOpen, loadGroups]);

  const filteredGroups = groups.filter((g) =>
    g.name.toLowerCase().includes(searchQuery.trim().toLowerCase()),
  );

  return (
    <>
      <Dialog open={isOpen} onOpenChange={(open) => !open && handleClose()}>
        <DialogContent className="max-w-md p-0 overflow-hidden sm:rounded-2xl border-border bg-card shadow-2xl">
          {/* Header */}
          <DialogHeader className="p-5 pb-3 border-b border-border/60">
            <div className="flex items-center gap-2.5">
              <div className="flex size-9 items-center justify-center rounded-xl bg-primary/10 text-primary">
                <LuArrowRightLeft className="size-4" />
              </div>
              <div>
                <DialogTitle className="text-base font-semibold">
                  {isVi
                    ? "Chuyển Nhóm Profile"
                    : "Move / Assign Profiles to Group"}
                </DialogTitle>
                <DialogDescription className="text-xs text-muted-foreground mt-0.5">
                  {isVi
                    ? `Đang chọn ${selectedProfiles.length} profile để chuyển sang nhóm đích`
                    : `Moving ${selectedProfiles.length} selected profile(s) to destination group`}
                </DialogDescription>
              </div>
            </div>
          </DialogHeader>

          <div className="p-5 space-y-4">
            {/* Search and Quick Action */}
            <div className="flex items-center gap-2">
              <div className="relative flex-1">
                <LuSearch className="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input
                  type="text"
                  placeholder={
                    isVi
                      ? "Tìm kiếm nhóm đích..."
                      : "Search destination group..."
                  }
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="h-9 pl-8 text-xs bg-muted/40"
                />
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-9 gap-1.5 text-xs shrink-0"
                onClick={() => {
                  setError(null);
                  setShowCreateGroup((visible) => !visible);
                }}
                disabled={isAssigning || isCreatingGroup}
              >
                <LuFolderPlus className="size-3.5" />
                <span>
                  {showCreateGroup
                    ? t("common.buttons.cancel")
                    : isVi
                      ? "Tạo nhóm mới"
                      : "New Group"}
                </span>
              </Button>
            </div>

            {showCreateGroup && (
              <div className="space-y-3 rounded-lg border border-border bg-muted/20 p-3">
                <div className="space-y-1.5">
                  <Label htmlFor="profile-group-inline-name">
                    {t("groups.form.name")}
                  </Label>
                  <Input
                    id="profile-group-inline-name"
                    autoFocus
                    value={newGroupName}
                    onChange={(event) => setNewGroupName(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        void handleCreateGroup();
                      }
                    }}
                    placeholder={t("groups.form.namePlaceholder")}
                    maxLength={128}
                    disabled={isCreatingGroup}
                  />
                </div>
                <div className="flex justify-end">
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => void handleCreateGroup()}
                    disabled={!newGroupName.trim() || isCreatingGroup}
                  >
                    {isCreatingGroup && (
                      <span className="mr-2 size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
                    )}
                    {t("common.buttons.create")}
                  </Button>
                </div>
              </div>
            )}

            {/* Destination Group Selection List */}
            <div className="space-y-1.5">
              <p className="text-xs font-medium text-foreground">
                {isVi ? "Chọn nhóm đích:" : "Select destination group:"}
              </p>

              <ScrollArea className="h-48 rounded-xl border border-border/80 bg-muted/20 p-1.5">
                {isLoading ? (
                  <div className="flex h-36 items-center justify-center text-xs text-muted-foreground">
                    <span className="size-4 animate-spin rounded-full border-2 border-primary border-t-transparent mr-2" />
                    {isVi ? "Đang tải danh sách nhóm..." : "Loading groups..."}
                  </div>
                ) : (
                  <div className="space-y-1">
                    {/* Option: No Group / Remove from group */}
                    <button
                      type="button"
                      onClick={() => setSelectedGroupId(null)}
                      className={cn(
                        "flex w-full items-center justify-between rounded-lg px-3 py-2 text-left text-xs transition-colors",
                        selectedGroupId === null
                          ? "bg-primary text-primary-foreground font-medium shadow-xs"
                          : "hover:bg-muted/60 text-foreground",
                      )}
                    >
                      <div className="flex items-center gap-2.5 truncate">
                        <span className="size-2 rounded-full bg-zinc-400 shrink-0" />
                        <span className="truncate inline-flex items-center gap-1.5">
                          <LuFolder className="size-3.5 text-muted-foreground" />
                          {isVi
                            ? "Chưa phân nhóm (Mặc định)"
                            : "Unassigned / No Group"}
                        </span>
                      </div>
                      {selectedGroupId === null && (
                        <LuCheck className="size-4 shrink-0" />
                      )}
                    </button>

                    {/* Group options */}
                    {filteredGroups.map((group) => {
                      const isSelected = selectedGroupId === group.id;
                      return (
                        <button
                          key={group.id}
                          type="button"
                          onClick={() => setSelectedGroupId(group.id)}
                          className={cn(
                            "flex w-full items-center justify-between rounded-lg px-3 py-2 text-left text-xs transition-colors",
                            isSelected
                              ? "bg-primary text-primary-foreground font-medium shadow-xs"
                              : "hover:bg-muted/60 text-foreground",
                          )}
                        >
                          <div className="flex items-center gap-2.5 truncate">
                            <span
                              className="size-2.5 rounded-full shrink-0"
                              style={{
                                backgroundColor: group.color || "#3b82f6",
                              }}
                            />
                            <span className="truncate">{group.name}</span>
                          </div>
                          {isSelected && (
                            <LuCheck className="size-4 shrink-0" />
                          )}
                        </button>
                      );
                    })}

                    {filteredGroups.length === 0 && searchQuery && (
                      <div className="py-6 text-center text-xs text-muted-foreground">
                        {isVi
                          ? `Không tìm thấy nhóm "${searchQuery}"`
                          : `No group matching "${searchQuery}"`}
                      </div>
                    )}
                  </div>
                )}
              </ScrollArea>
            </div>

            {/* Informational Callout (User question resolution) */}
            <div className="flex items-start gap-2.5 rounded-xl border border-border/80 bg-muted/40 p-3 text-[11px] text-muted-foreground leading-relaxed">
              <LuInfo className="size-4 text-primary shrink-0 mt-0.5" />
              <p>
                {isVi
                  ? "Profile sau khi chuyển nhóm vẫn luôn hiển thị trong bộ lọc 'Tất cả' (All). Đồng thời, profile sẽ thừa hưởng quyền truy cập thành viên và cấu hình proxy mặc định của nhóm đích."
                  : "Profiles moved to another group will always remain visible under the 'All' filter, and will inherit the destination group's member permissions and default proxy configuration."}
              </p>
            </div>

            {error && (
              <div className="rounded-xl border border-destructive/30 bg-destructive/10 p-3 text-xs text-destructive">
                {error}
              </div>
            )}
          </div>

          {/* Footer Actions */}
          <DialogFooter className="border-t border-border/60 bg-muted/20 px-5 py-3 gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleClose}
              disabled={isAssigning || isCreatingGroup}
              className="text-xs"
            >
              {t("common.buttons.cancel", { defaultValue: "Hủy" })}
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={() => void handleAssign()}
              disabled={isLoading || isAssigning || isCreatingGroup}
              className="text-xs gap-1.5"
            >
              {isAssigning && (
                <span className="size-3.5 animate-spin rounded-full border-2 border-primary-foreground border-t-transparent" />
              )}
              <span>{isVi ? "Xác nhận chuyển nhóm" : "Confirm Move"}</span>
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
