"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { LoadingButton } from "@/components/loading-button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { BrowserProfile, ExtensionGroup } from "@/types";
import { RippleButton } from "./ui/ripple";

interface ExtensionGroupAssignmentDialogProps {
  isOpen: boolean;
  onClose: () => void;
  workspaceId: string | null;
  selectedProfiles: string[];
  onAssignmentComplete: () => void;
  profiles?: BrowserProfile[];
}

export function ExtensionGroupAssignmentDialog({
  isOpen,
  onClose,
  workspaceId,
  selectedProfiles,
  onAssignmentComplete,
  profiles = [],
}: ExtensionGroupAssignmentDialogProps) {
  const { t } = useTranslation();
  const [groups, setGroups] = useState<ExtensionGroup[]>([]);
  const [selectedGroupId, setSelectedGroupId] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [isAssigning, setIsAssigning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [groupsError, setGroupsError] = useState<string | null>(null);
  const [completedProfiles, setCompletedProfiles] = useState<string[]>([]);
  const [failedProfiles, setFailedProfiles] = useState<string[]>([]);

  const loadGroups = useCallback(async () => {
    setIsLoading(true);
    setGroupsError(null);
    try {
      if (!workspaceId) throw new Error("Select a workspace first.");
      const groupList = await invoke<ExtensionGroup[]>(
        "list_extension_groups",
        { workspaceId },
      );
      setGroups(groupList);
    } catch (err) {
      console.error("Failed to load extension groups:", err);
      setGroupsError(
        err instanceof Error ? err.message : t("extensions.loadGroupsFailed"),
      );
    } finally {
      setIsLoading(false);
    }
  }, [workspaceId, t]);

  const handleAssign = useCallback(async () => {
    setIsAssigning(true);
    setError(null);
    const pendingProfiles = failedProfiles.length
      ? failedProfiles
      : selectedProfiles.filter(
          (profileId) => !completedProfiles.includes(profileId),
        );
    const failures: string[] = [];
    const failureReasons: string[] = [];
    for (const profileId of pendingProfiles) {
      try {
        await invoke("assign_extension_group_to_profile", {
          workspaceId,
          profileId,
          extensionGroupId:
            selectedGroupId === "__no_group__" ? null : selectedGroupId,
        });
        setCompletedProfiles((current) =>
          current.includes(profileId) ? current : [...current, profileId],
        );
      } catch (err) {
        failures.push(profileId);
        const reason =
          err &&
          typeof err === "object" &&
          "code" in err &&
          err.code === "api_unavailable"
            ? t("extensions.apiUnavailable")
            : err instanceof Error
              ? err.message
              : typeof err === "string"
                ? err
                : err && typeof err === "object" && "title" in err
                  ? String(err.title)
                  : t("extensions.assignGroupFailed");
        if (!failureReasons.includes(reason)) failureReasons.push(reason);
      }
    }
    setFailedProfiles(failures);
    if (failures.length) {
      setError(
        t("extensions.assignPartial", {
          succeeded:
            completedProfiles.length + pendingProfiles.length - failures.length,
          failed: failures.length,
          reason: failureReasons[0] ?? t("extensions.assignGroupFailed"),
        }),
      );
    } else {
      toast.success(
        t("extensions.assignSuccess", {
          count: completedProfiles.length + pendingProfiles.length,
        }),
      );
      onAssignmentComplete();
      onClose();
    }
    setIsAssigning(false);
  }, [
    selectedProfiles,
    selectedGroupId,
    completedProfiles,
    failedProfiles,
    workspaceId,
    onAssignmentComplete,
    onClose,
    t,
  ]);

  useEffect(() => {
    if (isOpen) {
      void loadGroups();
      setSelectedGroupId("");
      setCompletedProfiles([]);
      setFailedProfiles([]);
      setError(null);
      setGroupsError(null);
    }
  }, [isOpen, loadGroups]);

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{t("extensions.assignTitle")}</DialogTitle>
          <DialogDescription>
            {t("extensions.assignDescription", {
              count: selectedProfiles.length,
            })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-2">
            <Label>{t("extensions.assignTitle")}:</Label>
            <div className="max-h-[min(8rem,20vh)] overflow-y-auto rounded-md bg-muted p-3">
              <ul className="space-y-1 text-sm">
                {selectedProfiles.map((profileId) => {
                  const profile = profiles.find(
                    (p: BrowserProfile) => p.id === profileId,
                  );
                  const displayName =
                    profile?.name || t("extensions.profileUnavailable");
                  return (
                    <li
                      key={profileId}
                      className="flex min-w-0 items-center gap-2 truncate"
                    >
                      {completedProfiles.includes(profileId) ? (
                        <>
                          <span className="text-primary" aria-hidden="true">
                            ✓
                          </span>
                          <span className="sr-only">
                            {t("extensions.assignmentDone")}
                          </span>
                        </>
                      ) : failedProfiles.includes(profileId) ? (
                        <>
                          <span className="text-destructive" aria-hidden="true">
                            !
                          </span>
                          <span className="sr-only">
                            {t("extensions.assignmentFailed")}
                          </span>
                        </>
                      ) : null}
                      • {displayName}
                    </li>
                  );
                })}
              </ul>
            </div>
          </div>

          <div className="space-y-2">
            <Label htmlFor="extension-group-select">
              {t("extensions.extensionGroup")}:
            </Label>
            {isLoading ? (
              <div className="text-sm text-muted-foreground">
                {t("common.buttons.loading")}
              </div>
            ) : (
              <Select
                value={selectedGroupId || undefined}
                onValueChange={setSelectedGroupId}
              >
                <SelectTrigger disabled={isAssigning}>
                  <SelectValue placeholder={t("extensions.chooseGroup")} />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="__no_group__">
                    {t("extensions.noGroup")}
                  </SelectItem>
                  {groups.map((group) => (
                    <SelectItem key={group.id} value={group.id}>
                      {group.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
            {groupsError ? (
              <div
                role="alert"
                className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
              >
                <span className="min-w-0 break-words">{groupsError}</span>
                <RippleButton
                  variant="outline"
                  onClick={() => void loadGroups()}
                  disabled={isLoading || isAssigning}
                >
                  {t("common.buttons.retry")}
                </RippleButton>
              </div>
            ) : null}
          </div>

          {error && (
            <div
              role="alert"
              className="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
            >
              {error}
              {failedProfiles.length > 0 ? (
                <p className="mt-1 text-xs">
                  {t("extensions.retryOnlyFailed", {
                    count: failedProfiles.length,
                  })}
                </p>
              ) : null}
            </div>
          )}
        </div>

        <DialogFooter>
          <RippleButton
            variant="outline"
            onClick={onClose}
            disabled={isAssigning}
          >
            {t("common.buttons.cancel")}
          </RippleButton>
          <LoadingButton
            isLoading={isAssigning}
            onClick={() => void handleAssign()}
            disabled={
              isLoading ||
              Boolean(groupsError) ||
              selectedGroupId === "" ||
              selectedProfiles.length === 0
            }
          >
            {failedProfiles.length > 0
              ? t("extensions.retryFailedProfiles")
              : t("common.buttons.apply")}
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
