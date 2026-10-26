"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrent } from "@tauri-apps/plugin-deep-link";
import { motion } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AboutDialog } from "@/components/about-dialog";
import AppUpdateDialog from "@/components/app-update-dialog";
import { AuditLogView } from "@/components/audit-log-view";
import { CloneProfileDialog } from "@/components/clone-profile-dialog";
import { CloseConfirmDialog } from "@/components/close-confirm-dialog";
import { CommandPalette } from "@/components/command-palette";
import {
  type ConsistencyResult,
  ConsistencyWarningDialog,
  isConsistencyWarningSuppressed,
} from "@/components/consistency-warning-dialog";
import { CookieCopyDialog } from "@/components/cookie-copy-dialog";
import { CookieManagementDialog } from "@/components/cookie-management-dialog";
import { CreateProfileDialog } from "@/components/create-profile-dialog";
import { DeleteConfirmationDialog } from "@/components/delete-confirmation-dialog";
import { ExtensionGroupAssignmentDialog } from "@/components/extension-group-assignment-dialog";
import { ExtensionManagementDialog } from "@/components/extension-management-dialog";
import { GroupAssignmentDialog } from "@/components/group-assignment-dialog";
import HomeHeader from "@/components/home-header";
import InlineUpdateBanner from "@/components/inline-update-banner";
import { IntegrationsDialog } from "@/components/integrations-dialog";
import { AccountDirectory } from "@/components/management/accounts";
import { WorkspaceMembers } from "@/components/management/members";
import { OrganizationManagement } from "@/components/management/org-workspaces";
import { PersonalAccount } from "@/components/management/personal-account";
import { PlatformManagement } from "@/components/management/platform";
import { ManagementResources } from "@/components/management/resources";
import { PendingInvitesDialog } from "@/components/pending-invites-dialog";
import { ProfilesDataTable } from "@/components/profile-data-table";
import {
  type PasswordDialogMode,
  ProfilePasswordDialog,
} from "@/components/profile-password-dialog";
import { ProfileSelectorDialog } from "@/components/profile-selector-dialog";
import { ProxyAssignmentDialog } from "@/components/proxy-assignment-dialog";
import { ProxyManagementDialog } from "@/components/proxy-management-dialog";
import { type AppPage, RailNav } from "@/components/rail-nav";
import { SettingsDialog } from "@/components/settings-dialog";
import { ShortcutsPage } from "@/components/shortcuts-page";
import { SyncFollowerDialog } from "@/components/sync-follower-dialog";
import { TableFilterControls } from "@/components/table-controls";
import { TeamManagerView } from "@/components/team-manager-view";
import { WayfernConfigDialog } from "@/components/wayfern-config-dialog";
import { WayfernTermsDialog } from "@/components/wayfern-terms-dialog";
import { type WorkspaceItem } from "@/components/workspace-switcher";
import type { useAppUpdateNotifications } from "@/hooks/use-app-update-notifications";
import { useBrowserDownload } from "@/hooks/use-browser-download";
import { useCloudAuth } from "@/hooks/use-cloud-auth";
import { useGroupEvents } from "@/hooks/use-group-events";
import { useProfileEvents } from "@/hooks/use-profile-events";
import { useProfileTableFilters } from "@/hooks/use-profile-table-filters";
import { useProxyEvents } from "@/hooks/use-proxy-events";
import { useSyncSessions } from "@/hooks/use-sync-session";
import { useWayfernTerms } from "@/hooks/use-wayfern-terms";
import { translateBackendError } from "@/lib/backend-errors";
import { getEntitlements } from "@/lib/entitlements";
import { extractRootError, formatError } from "@/lib/error-utils";
import { MOTION_EASE_OUT } from "@/lib/motion";
import type { ProfileSearchContext } from "@/lib/profile-search";
import {
  hasProfilesForWorkspace,
  visibleProfilesForWorkspace,
  workspaceProfileCount,
} from "@/lib/profile-workspace-state";
import {
  matchesGroupDigit,
  matchesShortcut,
  SHORTCUTS,
  type ShortcutId,
} from "@/lib/shortcuts";
import {
  dismissToast,
  showErrorToast,
  showSuccessToast,
  showSyncProgressToast,
  showToast,
} from "@/lib/toast-utils";
import type { BrowserProfile, WayfernConfig } from "@/types";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import { WebRouteHost } from "./web-route-host";

type BrowserTypeString = "wayfern";

const desktopApi = createTauriDesktopApi();

type ShareProfilesHandler = (
  profiles: BrowserProfile[],
  workspaceId: string,
) => void;

interface PendingUrl {
  id: string;
  url: string;
}

export default function Home({
  onShareProfiles,
  onCheckForUpdates,
  isCheckingForUpdates,
  appUpdates,
}: {
  onShareProfiles?: ShareProfilesHandler;
  onCheckForUpdates?: () => void;
  isCheckingForUpdates?: boolean;
  appUpdates?: ReturnType<typeof useAppUpdateNotifications>;
} = {}) {
  return process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "web" ? (
    <WebRouteHost initialPath="/" />
  ) : (
    <DesktopHome
      onShareProfiles={onShareProfiles}
      onCheckForUpdates={onCheckForUpdates}
      isCheckingForUpdates={isCheckingForUpdates}
      appUpdates={appUpdates}
    />
  );
}

function DesktopHome({
  onShareProfiles,
  onCheckForUpdates,
  isCheckingForUpdates,
  appUpdates,
}: {
  onShareProfiles?: ShareProfilesHandler;
  onCheckForUpdates?: () => void;
  isCheckingForUpdates?: boolean;
  appUpdates?: ReturnType<typeof useAppUpdateNotifications>;
} = {}) {
  const { t } = useTranslation();
  // Use the profile events hook for centralized profile management
  const {
    profiles: loadedProfiles,
    runningProfiles,
    isLoading: profilesLoading,
    isRefreshing: profilesRefreshing,
    error: profilesError,
    loadProfiles: refreshProfiles,
    clearError: clearProfileError,
    loadGroups,
  } = useProfileEvents();
  const [workspaces, setWorkspaces] = useState<WorkspaceItem[]>([]);
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState("");
  const [profilesWorkspaceId, setProfilesWorkspaceId] = useState<string | null>(
    null,
  );
  const [workspaceProfilesLoading, setWorkspaceProfilesLoading] =
    useState(false);
  const [workspaceProfilesFailed, setWorkspaceProfilesFailed] = useState(false);
  const [workspaceSwitching, setWorkspaceSwitching] = useState(false);
  const workspaceSelectionRequestRef = useRef(0);
  const [workspaceLoading, setWorkspaceLoading] = useState(true);
  const [workspaceListRevision, setWorkspaceListRevision] = useState(0);
  const refreshWorkspaceList = useCallback(() => {
    setWorkspaceListRevision((revision) => revision + 1);
  }, []);
  const [pendingInvitesOpen, setPendingInvitesOpen] = useState(false);
  const [pendingInviteLink, setPendingInviteLink] = useState("");

  // Keep window/body scroll position pinned to 0 so no programmatic or drag action causes horizontal scrolling
  useEffect(() => {
    const pin = () => {
      if (document.body.scrollLeft !== 0) document.body.scrollLeft = 0;
      if (document.documentElement.scrollLeft !== 0) {
        document.documentElement.scrollLeft = 0;
      }
    };
    pin();
    window.addEventListener("scroll", pin, true);
    return () => window.removeEventListener("scroll", pin, true);
  }, []);

  const profilesReadyForWorkspace = hasProfilesForWorkspace(
    selectedWorkspaceId,
    profilesWorkspaceId,
    workspaceSwitching,
  );
  const activeWorkspace = useMemo(
    () =>
      workspaces.find((workspace) => workspace.id === selectedWorkspaceId) ??
      null,
    [workspaces, selectedWorkspaceId],
  );
  const profiles = visibleProfilesForWorkspace(
    loadedProfiles,
    profilesReadyForWorkspace,
  );

  // Keep the active workspace count tied only to a successful profile response
  useEffect(() => {
    if (!profilesReadyForWorkspace || profilesLoading) return;
    setWorkspaces((prev) =>
      prev.map((ws) =>
        ws.id === selectedWorkspaceId
          ? { ...ws, profileCount: profiles.length, selected: true }
          : { ...ws, selected: false },
      ),
    );
  }, [
    selectedWorkspaceId,
    profiles.length,
    profilesLoading,
    profilesReadyForWorkspace,
  ]);

  const handleWorkspaceSelect = useCallback(
    async (workspaceId: string) => {
      const request = ++workspaceSelectionRequestRef.current;
      setWorkspaceSwitching(true);
      setWorkspaceProfilesLoading(true);
      setWorkspaceProfilesFailed(false);
      clearProfileError();
      setSelectedProfiles([]);
      setSelectedProfilesForCookies([]);
      setSelectedProfilesForGroup([]);
      setSelectedProfilesForProxy([]);
      setSelectedProfilesForExtensionGroup([]);
      setSelectedGroupId("__all__");
      setProfileInfoDialog(null);
      setCurrentProfileForCookieManagement(null);
      setCookieManagementDialogOpen(false);
      setCookieCopyDialogOpen(false);
      setCurrentProfileForWayfernConfig(null);
      setWayfernConfigDialogOpen(false);
      setCloneProfile(null);
      setPasswordDialogProfile(null);
      setConsistencyWarning(null);
      setGroupAssignmentDialogOpen(false);
      setExtensionGroupAssignmentDialogOpen(false);
      setProxyAssignmentDialogOpen(false);
      setPendingBulkAction(null);
      setShowBulkDeleteConfirmation(false);
      try {
        await invoke("select_active_workspace", { workspaceId });
        if (request !== workspaceSelectionRequestRef.current) return;
        setSelectedWorkspaceId(workspaceId);
        setProfilesWorkspaceId(null);
        const [profilesLoaded] = await Promise.all([
          refreshProfiles(),
          loadGroups(),
        ]);
        if (request !== workspaceSelectionRequestRef.current) return;
        if (profilesLoaded) {
          setProfilesWorkspaceId(workspaceId);
        } else {
          setWorkspaceProfilesFailed(true);
          showErrorToast(
            t("errors.loadProfilesFailed", {
              error: t("errors.loadWorkspaceFailed"),
            }),
          );
        }
      } catch (error) {
        if (request === workspaceSelectionRequestRef.current) {
          showErrorToast(t("errors.loadWorkspaceFailed"));
        }
        console.error("Workspace selection failed:", error);
      } finally {
        if (request === workspaceSelectionRequestRef.current) {
          setWorkspaceSwitching(false);
          setWorkspaceProfilesLoading(false);
        }
      }
    },
    [t, refreshProfiles, loadGroups, clearProfileError],
  );

  const refreshSelectedWorkspaceProfiles = useCallback(async () => {
    const workspaceId = selectedWorkspaceId;
    if (!workspaceId) return;
    const request = workspaceSelectionRequestRef.current;
    setWorkspaceProfilesLoading(true);
    setWorkspaceProfilesFailed(false);
    clearProfileError();
    try {
      const loaded = await refreshProfiles();
      if (
        request !== workspaceSelectionRequestRef.current ||
        workspaceId !== selectedWorkspaceId
      ) {
        return;
      }
      if (loaded) {
        setProfilesWorkspaceId(workspaceId);
      } else {
        setWorkspaceProfilesFailed(true);
        showErrorToast(
          t("errors.loadProfilesFailed", {
            error: t("errors.loadWorkspaceFailed"),
          }),
        );
      }
    } finally {
      if (request === workspaceSelectionRequestRef.current) {
        setWorkspaceProfilesLoading(false);
      }
    }
  }, [selectedWorkspaceId, refreshProfiles, clearProfileError, t]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: workspaceListRevision explicitly reloads data after organization/workspace mutations.
  useEffect(() => {
    let disposed = false;
    setWorkspaceLoading(true);
    void (async () => {
      try {
        const available = await invoke<Array<any>>("get_available_workspaces");
        if (disposed) return;

        // Fetch organizations to resolve human-readable names
        const orgNamesMap = new Map<string, string>();
        try {
          const orgPage = await desktopApi.listOrganizations();
          if (orgPage?.items) {
            for (const org of orgPage.items) {
              orgNamesMap.set(org.id, org.name);
            }
          }
        } catch {
          // ignore
        }

        if (available && available.length > 0) {
          const mapped: WorkspaceItem[] = available.map((w: any) => ({
            id: w.id,
            name: w.name,
            organizationId: w.organizationId,
            organizationName:
              w.organizationName ||
              orgNamesMap.get(w.organizationId) ||
              (w.organizationId
                ? t("header.mainOrg", { defaultValue: "Tổ Chức Chính" })
                : t("header.personalSpace", {
                    defaultValue: "Không Gian Cá Nhân",
                  })),
            maxProfiles:
              typeof w.policy?.maxProfiles === "number"
                ? w.policy.maxProfiles
                : undefined,
            maxMembers:
              typeof w.policy?.maxMembers === "number"
                ? w.policy.maxMembers
                : undefined,
            selected: w.selected,
            type: w.organizationId ? "team" : "personal",
            role: w.role,
            profileCount:
              typeof w.profileCount === "number" ? w.profileCount : undefined,
          }));
          setWorkspaces(mapped);
          const selected =
            mapped.find((workspace) => workspace.selected) || mapped[0];
          if (selected) {
            setSelectedWorkspaceId(selected.id);
            setWorkspaceProfilesLoading(true);
            clearProfileError();
            const profilesLoaded = await refreshProfiles();
            if (disposed) return;
            if (profilesLoaded) {
              setProfilesWorkspaceId(selected.id);
              setWorkspaceProfilesFailed(false);
            } else {
              setWorkspaceProfilesFailed(true);
              showErrorToast(
                t("errors.loadProfilesFailed", {
                  error: t("errors.loadWorkspaceFailed"),
                }),
              );
            }
            setWorkspaceProfilesLoading(false);
          }
        } else {
          setWorkspaces([]);
          setSelectedWorkspaceId("");
          setProfilesWorkspaceId(null);
          setWorkspaceProfilesFailed(false);
        }
      } catch (error: unknown) {
        if (!disposed) {
          console.warn("Workspace list remote init error:", error);
          setWorkspaces([]);
          setSelectedWorkspaceId("");
          setProfilesWorkspaceId(null);
        }
      } finally {
        if (!disposed) setWorkspaceLoading(false);
      }
    })();
    return () => {
      disposed = true;
    };
  }, [t, refreshProfiles, clearProfileError, workspaceListRevision]);
  const { downloadBrowser, loadDownloadedVersions, loadVersions } =
    useBrowserDownload();

  // Auto-preload the default browser runtime on app startup
  useEffect(() => {
    let cancelled = false;
    const preloadRuntime = async () => {
      try {
        const installed = await loadDownloadedVersions("wayfern");
        if (cancelled || (Array.isArray(installed) && installed.length > 0))
          return;
        const releases = await loadVersions("wayfern");
        if (cancelled || !releases || releases.length === 0) return;
        const latest = releases[0]?.tag_name;
        if (latest && !installed.includes(latest)) {
          void downloadBrowser("wayfern", latest, false);
        }
      } catch (err) {
        console.warn("Background browser runtime preload skipped:", err);
      }
    };
    void preloadRuntime();
    return () => {
      cancelled = true;
    };
  }, [loadDownloadedVersions, loadVersions, downloadBrowser]);

  // Startup onboarding acceptance and permission prompts are intentionally
  // omitted; optional permissions are requested when their features are used.

  const {
    groups: groupsData,
    isLoading: groupsLoading,
    error: groupsError,
  } = useGroupEvents();

  const {
    storedProxies,
    isLoading: proxiesLoading,
    error: proxiesError,
  } = useProxyEvents();

  // Synchronizer sessions
  const { getProfileSyncInfo } = useSyncSessions();
  const [syncLeaderProfile, setSyncLeaderProfile] =
    useState<BrowserProfile | null>(null);

  // Wayfern terms are enforced by its runtime; BugLogin does not use Donut trial state.
  const {
    termsAccepted,
    isLoading: termsLoading,
    checkTerms,
  } = useWayfernTerms();
  // Cloud auth for cross-OS unlock
  const { user: cloudUser } = useCloudAuth();
  const crossOsUnlocked = getEntitlements(cloudUser).crossOsFingerprints;
  // Bulk run/stop is a paid (browser automation) feature, matching the
  // /v1/profiles/batch/run API gate. Free/starter users see the bulk Run/Stop
  // actions disabled with a Pro badge.
  const automationUnlocked = getEntitlements(cloudUser).browserAutomation;

  const _syncUnlocked = crossOsUnlocked;

  const [currentPage, setCurrentPage] = useState<AppPage>("profiles");
  // Tracks which tab inside the shared proxy-management page should be active.
  const [proxyManagementInitialTab, setProxyManagementInitialTab] =
    useState<"proxies">("proxies");
  const [integrationsInitialTab, setIntegrationsInitialTab] = useState<
    "api" | "remote"
  >("api");
  const [createProfileDialogOpen, setCreateProfileDialogOpen] = useState(false);
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);
  const [integrationsDialogOpen, setIntegrationsDialogOpen] = useState(false);
  const [proxyManagementDialogOpen, setProxyManagementDialogOpen] =
    useState(false);
  const [wayfernConfigDialogOpen, setWayfernConfigDialogOpen] = useState(false);
  const [extensionManagementDialogOpen, setExtensionManagementDialogOpen] =
    useState(false);
  const [groupAssignmentDialogOpen, setGroupAssignmentDialogOpen] =
    useState(false);
  const [
    extensionGroupAssignmentDialogOpen,
    setExtensionGroupAssignmentDialogOpen,
  ] = useState(false);
  const [
    selectedProfilesForExtensionGroup,
    setSelectedProfilesForExtensionGroup,
  ] = useState<string[]>([]);
  const [proxyAssignmentDialogOpen, setProxyAssignmentDialogOpen] =
    useState(false);
  const [cookieCopyDialogOpen, setCookieCopyDialogOpen] = useState(false);
  const [cookieManagementDialogOpen, setCookieManagementDialogOpen] =
    useState(false);
  const [
    currentProfileForCookieManagement,
    setCurrentProfileForCookieManagement,
  ] = useState<BrowserProfile | null>(null);
  const [selectedProfilesForCookies, setSelectedProfilesForCookies] = useState<
    string[]
  >([]);
  const [selectedGroupId, setSelectedGroupId] = useState<string>("__all__");
  const [selectedProfilesForGroup, setSelectedProfilesForGroup] = useState<
    string[]
  >([]);
  const [selectedProfilesForProxy, setSelectedProfilesForProxy] = useState<
    string[]
  >([]);
  const [selectedProfiles, setSelectedProfiles] = useState<string[]>([]);
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [pendingUrls, setPendingUrls] = useState<PendingUrl[]>([]);
  const [currentProfileForWayfernConfig, setCurrentProfileForWayfernConfig] =
    useState<BrowserProfile | null>(null);
  const [cloneProfile, setCloneProfile] = useState<BrowserProfile | null>(null);
  const [passwordDialogProfile, setPasswordDialogProfile] =
    useState<BrowserProfile | null>(null);
  const [passwordDialogMode, setPasswordDialogMode] =
    useState<PasswordDialogMode>("set");
  const pendingLaunchAfterUnlockRef = useRef<BrowserProfile | null>(null);
  const runtimeInstallTasksRef = useRef(new Map<string, Promise<void>>());
  const [showBulkDeleteConfirmation, setShowBulkDeleteConfirmation] =
    useState(false);
  const [isBulkDeleting, setIsBulkDeleting] = useState(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [aboutDialogOpen, setAboutDialogOpen] = useState(false);
  const [consistencyWarning, setConsistencyWarning] = useState<{
    profile: BrowserProfile;
    result: ConsistencyResult;
  } | null>(null);
  // Owned by page.tsx so the command palette can request opening the profile
  // info dialog. ProfilesDataTable consumes it through controlled props.
  const [profileInfoDialog, setProfileInfoDialog] =
    useState<BrowserProfile | null>(null);

  const handleSelectGroup = useCallback((groupId: string) => {
    setSelectedGroupId(groupId);
    setSelectedProfiles([]);
  }, []);

  const handleRailNavigate = useCallback(
    (page: AppPage) => {
      if (!selectedWorkspaceId && ["groups", "proxies"].includes(page)) {
        showErrorToast(t("errors.selectWorkspaceFirst"));
        return;
      }
      // Always reset every sub-page-able dialog before opening the next one,
      // so navigating from one rail item to another doesn't stack two
      // sub-pages on top of each other.
      setSettingsDialogOpen(false);
      setProxyManagementDialogOpen(false);
      setExtensionManagementDialogOpen(false);
      setIntegrationsDialogOpen(false);

      setCurrentPage(page);
      switch (page) {
        case "profiles":
          break;
        case "settings":
          setSettingsDialogOpen(true);
          break;
        case "proxies":
          setProxyManagementInitialTab("proxies");
          setProxyManagementDialogOpen(true);
          break;
        case "extensions":
          setExtensionManagementDialogOpen(true);
          break;
        case "groups":
          break;
        case "integrations":
          setIntegrationsDialogOpen(true);
          break;
        case "account":
          break;
        case "shortcuts":
        case "accountManagement":
        case "orgCenter":
        case "teamManager":
        case "auditLogs":
          // Plain full-page view renders inside <main>
          break;
      }
    },
    [selectedWorkspaceId, t],
  );

  const runShortcut = useCallback(
    (id: ShortcutId) => {
      switch (id) {
        case "openPalette":
          setCommandPaletteOpen(true);
          break;
        case "openShortcuts":
          handleRailNavigate("shortcuts");
          break;
        case "goProfiles":
          handleRailNavigate("profiles");
          break;
        case "goProxies": {
          handleRailNavigate("proxies");
          break;
        }
        case "goExtensions": {
          handleRailNavigate("extensions");
          break;
        }
        case "goGroups":
          handleRailNavigate("groups");
          break;
        case "goIntegrations": {
          // Mod+I: flip api↔mcp tab when already on integrations.
          if (currentPage === "integrations") {
            setIntegrationsInitialTab((cur) =>
              cur === "api" ? "remote" : "api",
            );
          } else {
            handleRailNavigate("integrations");
          }
          break;
        }
        case "goAccount":
          handleRailNavigate("account");
          break;
        case "goSettings":
          handleRailNavigate("settings");
          break;
      }
    },
    [handleRailNavigate, currentPage],
  );

  // Ordered list the digit shortcuts and palette consume. "__all__" is index 1
  // so Mod+1 always lands on the unfiltered view; the user's groups follow.
  const orderedGroupTargets = useMemo(
    () => [
      { id: "__all__", name: t("rail.profiles") },
      ...groupsData.map((g) => ({ id: g.id, name: g.name })),
    ],
    [groupsData, t],
  );

  const selectGroupByDigit = useCallback(
    (digit: number) => {
      const target = orderedGroupTargets[digit - 1];
      if (!target) return;
      handleRailNavigate("profiles");
      handleSelectGroup(target.id);
    },
    [orderedGroupTargets, handleRailNavigate, handleSelectGroup],
  );

  useEffect(() => {
    // Global keydown — handles Mod+1..9 group jumps first, then falls back to
    // the static SHORTCUTS table. Skipped while typing in an input, EXCEPT
    // ⌘K and ⌘/ which are meta-level shortcuts and should always be reachable.
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName;
      const isTyping =
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        target?.isContentEditable === true;

      const digit = matchesGroupDigit(e);
      if (digit !== null) {
        if (isTyping) return;
        if (digit - 1 >= orderedGroupTargets.length) return;
        e.preventDefault();
        selectGroupByDigit(digit);
        return;
      }

      for (const s of SHORTCUTS) {
        if (!matchesShortcut(s, e)) continue;
        if (isTyping && s.id !== "openPalette" && s.id !== "openShortcuts") {
          return;
        }
        e.preventDefault();
        runShortcut(s.id);
        return;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [runShortcut, selectGroupByDigit, orderedGroupTargets.length]);

  const [processingUrls, setProcessingUrls] = useState<Set<string>>(new Set());

  const handleUrlOpen = useCallback(
    (url: string) => {
      // Prevent duplicate processing of the same URL
      if (processingUrls.has(url)) {
        console.log("URL already being processed:", url);
        return;
      }

      setProcessingUrls((prev) => new Set(prev).add(url));

      try {
        console.log("URL received for opening:", url);

        if (url.startsWith("buglogin://invite")) {
          setPendingInviteLink(url);
          setPendingInvitesOpen(true);
          return;
        }

        // Always show profile selector for manual selection - never auto-open
        // Replace any existing pending URL with the new one
        setPendingUrls([{ id: Date.now().toString(), url }]);
      } finally {
        // Remove URL from processing set after a short delay to prevent rapid duplicates
        setTimeout(() => {
          setProcessingUrls((prev) => {
            const next = new Set(prev);
            next.delete(url);
            return next;
          });
        }, 1000);
      }
    },
    [processingUrls],
  );

  // Browser updates use the signed BugLogin runtime authority once its
  // installer is available; do not expose Donut updater state here.
  const isUpdating = false;

  // Check for startup URLs but only process them once
  const [hasCheckedStartupUrl, setHasCheckedStartupUrl] = useState(false);
  const checkCurrentUrl = useCallback(async () => {
    if (hasCheckedStartupUrl) return;

    try {
      const currentUrl = await getCurrent();
      if (currentUrl && currentUrl.length > 0) {
        console.log("Startup URL detected:", currentUrl[0]);
        handleUrlOpen(currentUrl[0]);
      }
    } catch (error) {
      console.error("Failed to check current URL:", error);
    } finally {
      setHasCheckedStartupUrl(true);
    }
  }, [handleUrlOpen, hasCheckedStartupUrl]);

  // Handle profile errors from useProfileEvents hook
  useEffect(() => {
    if (
      profilesError &&
      !workspaceProfilesFailed &&
      !profilesError.includes("api_unavailable")
    ) {
      showErrorToast(profilesError);
    }
  }, [profilesError, workspaceProfilesFailed]);

  // Handle group errors from useGroupEvents hook
  useEffect(() => {
    if (groupsError && !groupsError.includes("api_unavailable")) {
      showErrorToast(groupsError);
    }
  }, [groupsError]);

  // Handle proxy errors from useProxyEvents hook
  useEffect(() => {
    if (proxiesError && !proxiesError.includes("api_unavailable")) {
      showErrorToast(proxiesError);
    }
  }, [proxiesError]);

  const listenForUrlEvents = useCallback(async () => {
    // Collect every listener we register so that — whether setup completes or
    // throws partway through — we tear down exactly what was registered.
    // Previously the Tauri unlisten handles were discarded (so re-runs stacked
    // duplicate handlers and a single URL was handled N times), and a failing
    // listen() call would leak the listeners that had already succeeded.
    const unlisteners: Array<() => void> = [];
    let handleLogoUrlEvent: ((event: CustomEvent) => void) | undefined;
    const teardown = () => {
      for (const unlisten of unlisteners) unlisten();
      if (handleLogoUrlEvent) {
        window.removeEventListener(
          "url-open-request",
          handleLogoUrlEvent as EventListener,
        );
      }
    };

    try {
      // Listen for URL open events from the deep link handler (when app is already running)
      unlisteners.push(
        await listen<string>("url-open-request", (event) => {
          console.log("Received URL open request:", event.payload);
          handleUrlOpen(event.payload);
        }),
      );
      unlisteners.push(
        await listen<string>("buglogin://deep-link", (event) => {
          handleUrlOpen(event.payload);
        }),
      );

      // Listen for show profile selector events
      unlisteners.push(
        await listen<string>("show-profile-selector", (event) => {
          console.log("Received show profile selector request:", event.payload);
          handleUrlOpen(event.payload);
        }),
      );

      // Listen for show create profile dialog events
      unlisteners.push(
        await listen<string>("show-create-profile-dialog", (event) => {
          console.log(
            "Received show create profile dialog request:",
            event.payload,
          );
          showErrorToast(t("errors.noProfilesForUrl"));
          setCreateProfileDialogOpen(true);
        }),
      );

      // Listen for custom logo click events
      handleLogoUrlEvent = (event: CustomEvent) => {
        console.log("Received logo URL event:", event.detail);
        handleUrlOpen(event.detail);
      };
      window.addEventListener(
        "url-open-request",
        handleLogoUrlEvent as EventListener,
      );

      return teardown;
    } catch (error) {
      console.error("Failed to setup URL listener:", error);
      // Tear down whatever did register before the failure so nothing leaks.
      teardown();
    }
  }, [handleUrlOpen, t]);

  const handleConfigureWayfern = useCallback((profile: BrowserProfile) => {
    setCurrentProfileForWayfernConfig(profile);
    setWayfernConfigDialogOpen(true);
  }, []);

  const handleSaveWayfernConfig = useCallback(
    async (profile: BrowserProfile, config: WayfernConfig) => {
      try {
        await invoke("update_wayfern_config", {
          profileId: profile.id,
          config,
        });
        // No need to manually reload - useProfileEvents will handle the update
        setWayfernConfigDialogOpen(false);
      } catch (err: unknown) {
        console.error("Failed to update wayfern config:", err);
        showErrorToast(
          t("errors.updateWayfernConfigFailed", { error: formatError(err) }),
        );
        throw err;
      }
    },
    [t],
  );

  const handleCreateProfile = useCallback(
    async (profileData: {
      name: string;
      idempotencyKey: string;
      browserStr: BrowserTypeString;
      version?: string;
      proxyId?: string;
      proxySettingsDraft?: {
        proxy_type: string;
        host: string;
        port: number;
        username: string;
        password: string;
      };
      vpnId?: string;
      wayfernConfig?: WayfernConfig;
      groupId?: string;
      extensionGroupId?: string;
      ephemeral?: boolean;
      dnsBlocklist?: string;
      launchHook?: string;
      password?: string;
    }) => {
      let profile: BrowserProfile;
      try {
        profile = await invoke<BrowserProfile>("create_browser_profile_new", {
          name: profileData.name,
          idempotencyKey: profileData.idempotencyKey,
          browserStr: profileData.browserStr,
          version: profileData.version,
          proxyId: profileData.proxyId,
          proxySettings: profileData.proxySettingsDraft,
          vpnId: profileData.vpnId,
          wayfernConfig: profileData.wayfernConfig,
          groupId:
            profileData.groupId ??
            (selectedGroupId && selectedGroupId !== "__all__"
              ? selectedGroupId
              : undefined),
          ephemeral: profileData.ephemeral,
          dnsBlocklist: profileData.dnsBlocklist,
          launchHook: profileData.launchHook,
        });
      } catch (error) {
        showErrorToast(
          t("errors.createProfileFailed", {
            error: translateBackendError(t, error),
          }),
        );
        throw error;
      }

      if (profileData.extensionGroupId) {
        try {
          await invoke("assign_extension_group_to_profile", {
            workspaceId: selectedWorkspaceId,
            profileId: profile.id,
            extensionGroupId: profileData.extensionGroupId,
          });
        } catch (error) {
          showErrorToast(
            t("errors.profileCreatedExtensionGroupFailed", {
              error: translateBackendError(t, error),
            }),
          );
          throw error;
        }
      }

      if (profileData.password && !profileData.ephemeral) {
        try {
          await invoke("set_profile_password", {
            profileId: profile.id,
            password: profileData.password,
          });
        } catch (error) {
          showErrorToast(
            t("errors.profileCreatedPasswordFailed", {
              error: translateBackendError(t, error),
            }),
          );
          throw error;
        }
      }

      // Immediately reload profiles to guarantee instant UI update
      try {
        await refreshProfiles();
      } catch (err) {
        console.warn("Immediate profile reload notice:", err);
      }
    },
    [selectedGroupId, selectedWorkspaceId, t, refreshProfiles],
  );

  const launchProfile = useCallback(
    async (profile: BrowserProfile) => {
      console.log("Starting launch for profile:", profile.name);

      // Password-protected: must be unlocked before launch
      if (profile.password_protected) {
        try {
          const isLocked = await invoke<boolean>("is_profile_locked", {
            profileId: profile.id,
          });
          if (isLocked) {
            pendingLaunchAfterUnlockRef.current = profile;
            setPasswordDialogMode("unlock");
            setPasswordDialogProfile(profile);
            return;
          }
        } catch (err) {
          console.error("Failed to check profile lock state:", err);
        }
      }

      try {
        if (profile.browser === "wayfern") {
          if (!profile.version) {
            throw new Error(t("errors.profileRuntimeVersionMissing"));
          }
          const version = profile.version;
          const installKey = `${profile.browser}:${profile.version}`;
          let installTask = runtimeInstallTasksRef.current.get(installKey);
          if (!installTask) {
            installTask = (async () => {
              const installed = await loadDownloadedVersions(profile.browser);
              if (!installed.includes(version)) {
                await downloadBrowser(profile.browser, version, true);
              }
            })();
            runtimeInstallTasksRef.current.set(installKey, installTask);
          }
          try {
            await installTask;
          } finally {
            if (
              runtimeInstallTasksRef.current.get(installKey) === installTask
            ) {
              runtimeInstallTasksRef.current.delete(installKey);
            }
          }
        }
        const result = await invoke<BrowserProfile>("launch_browser_profile", {
          profile,
        });
        console.log("Successfully launched profile:", result.name);

        // Non-blocking: after a successful launch, check that the proxy exit
        // node's timezone/country agrees with the fingerprint. A mismatch is a
        // strong anti-bot tell even though the real device never leaks.
        if (profile.proxy_id && !isConsistencyWarningSuppressed(profile.id)) {
          void invoke<ConsistencyResult>(
            "check_profile_fingerprint_consistency",
            { profileId: profile.id },
          )
            .then((res) => {
              if (res.checked && !res.consistent) {
                setConsistencyWarning({ profile, result: res });
              }
            })
            .catch((e) => {
              console.warn("Consistency check failed:", e);
            });
        }
      } catch (err: unknown) {
        console.error("Failed to launch browser:", err);
        const errorMessage = extractRootError(err);
        showErrorToast(
          t("errors.launchBrowserFailed", { error: errorMessage }),
        );
        throw err;
      }
    },
    [downloadBrowser, loadDownloadedVersions, t],
  );

  const handleCloneProfile = useCallback((profile: BrowserProfile) => {
    setCloneProfile(profile);
  }, []);

  const handleSetPassword = useCallback((profile: BrowserProfile) => {
    pendingLaunchAfterUnlockRef.current = null;
    setPasswordDialogMode("set");
    setPasswordDialogProfile(profile);
  }, []);

  const handleChangePassword = useCallback((profile: BrowserProfile) => {
    pendingLaunchAfterUnlockRef.current = null;
    setPasswordDialogMode("change");
    setPasswordDialogProfile(profile);
  }, []);

  const handleRemovePassword = useCallback((profile: BrowserProfile) => {
    pendingLaunchAfterUnlockRef.current = null;
    setPasswordDialogMode("remove");
    setPasswordDialogProfile(profile);
  }, []);

  const handleDeleteProfile = useCallback(
    async (profile: BrowserProfile) => {
      console.log("Attempting to delete profile:", profile.name);

      try {
        // First check if the browser is running for this profile
        const isRunning = await invoke<boolean>("check_browser_status", {
          profile,
        });

        if (isRunning) {
          showErrorToast(t("errors.cannotDeleteRunningProfile"));
          return;
        }

        // Attempt to delete the profile
        await invoke("delete_profile", { profileId: profile.id });
        console.log("Profile deletion command completed successfully");

        await refreshProfiles();
        console.log("Profile deleted successfully");
      } catch (err: unknown) {
        console.error("Failed to delete profile:", err);
        const errorMessage = formatError(err);
        showErrorToast(
          t("errors.deleteProfileFailed", { error: errorMessage }),
        );
      }
    },
    [t, refreshProfiles],
  );

  const handleRenameProfile = useCallback(
    async (profileId: string, newName: string) => {
      try {
        await invoke("rename_profile", { profileId, newName });
        await refreshProfiles();
      } catch (err: unknown) {
        console.error("Failed to rename profile:", err);
        showErrorToast(
          t("errors.renameProfileFailed", { error: formatError(err) }),
        );
        throw err;
      }
    },
    [t, refreshProfiles],
  );

  const handleKillProfile = useCallback(
    async (profile: BrowserProfile) => {
      console.log("Starting kill for profile:", profile.name);

      try {
        await invoke("kill_browser_profile", { profile });
        console.log("Successfully killed profile:", profile.name);
        await refreshProfiles();
      } catch (err: unknown) {
        console.error("Failed to kill browser:", err);
        const errorMessage = formatError(err);
        showErrorToast(t("errors.killBrowserFailed", { error: errorMessage }));
        // Re-throw the error so the table component can handle loading state cleanup
        throw err;
      }
    },
    [t, refreshProfiles],
  );

  const handleDeleteSelectedProfiles = useCallback(
    async (profileIds: string[]) => {
      try {
        await invoke("delete_selected_profiles", { profileIds });
        await refreshProfiles();
      } catch (err: unknown) {
        console.error("Failed to delete selected profiles:", err);
        showErrorToast(
          t("errors.deleteSelectedProfilesFailed", {
            error: formatError(err),
          }),
        );
      }
    },
    [t, refreshProfiles],
  );

  const handleAssignProfilesToGroup = useCallback((profileIds: string[]) => {
    setSelectedProfilesForGroup(profileIds);
    setGroupAssignmentDialogOpen(true);
  }, []);

  const handleBulkDelete = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    setShowBulkDeleteConfirmation(true);
  }, [selectedProfiles]);

  const confirmBulkDelete = useCallback(async () => {
    if (selectedProfiles.length === 0) return;

    setIsBulkDeleting(true);
    try {
      await invoke("delete_selected_profiles", {
        profileIds: selectedProfiles,
      });
      setSelectedProfiles([]);
      setShowBulkDeleteConfirmation(false);
      await refreshProfiles();
    } catch (error) {
      console.error("Failed to delete selected profiles:", error);
      showErrorToast(
        t("errors.deleteSelectedProfilesFailed", {
          error: JSON.stringify(error),
        }),
      );
    } finally {
      setIsBulkDeleting(false);
    }
  }, [selectedProfiles, t, refreshProfiles]);

  const handleBulkGroupAssignment = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    handleAssignProfilesToGroup(selectedProfiles);
    setSelectedProfiles([]);
  }, [selectedProfiles, handleAssignProfilesToGroup]);

  const handleAssignExtensionGroup = useCallback((profileIds: string[]) => {
    setSelectedProfilesForExtensionGroup(profileIds);
    setExtensionGroupAssignmentDialogOpen(true);
  }, []);

  const handleBulkExtensionGroupAssignment = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    handleAssignExtensionGroup(selectedProfiles);
    setSelectedProfiles([]);
  }, [selectedProfiles, handleAssignExtensionGroup]);

  const handleExtensionGroupAssignmentComplete = useCallback(() => {
    setExtensionGroupAssignmentDialogOpen(false);
    setSelectedProfilesForExtensionGroup([]);
  }, []);

  const handleAssignProfilesToProxy = useCallback((profileIds: string[]) => {
    setSelectedProfilesForProxy(profileIds);
    setProxyAssignmentDialogOpen(true);
  }, []);

  const handleBulkProxyAssignment = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    handleAssignProfilesToProxy(selectedProfiles);
    setSelectedProfiles([]);
  }, [selectedProfiles, handleAssignProfilesToProxy]);

  const handleBulkCopyCookies = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    const eligibleProfiles = profiles.filter(
      (p) => selectedProfiles.includes(p.id) && p.browser === "wayfern",
    );
    if (eligibleProfiles.length === 0) {
      showErrorToast(t("errors.cookieCopyUnsupportedBrowser"));
      return;
    }
    setSelectedProfilesForCookies(eligibleProfiles.map((p) => p.id));
    setCookieCopyDialogOpen(true);
  }, [selectedProfiles, profiles, t]);

  const [pendingBulkAction, setPendingBulkAction] = useState<{
    action: "run" | "stop";
    profiles: BrowserProfile[];
  } | null>(null);
  const [isBulkActing, setIsBulkActing] = useState(false);

  const executeBulkRun = useCallback(
    async (targets: BrowserProfile[]) => {
      setIsBulkActing(true);
      try {
        await Promise.allSettled(targets.map((p) => launchProfile(p)));
        setSelectedProfiles([]);
      } finally {
        setIsBulkActing(false);
        setPendingBulkAction(null);
      }
    },
    [launchProfile],
  );

  const executeBulkStop = useCallback(
    async (targets: BrowserProfile[]) => {
      setIsBulkActing(true);
      try {
        await Promise.allSettled(targets.map((p) => handleKillProfile(p)));
        setSelectedProfiles([]);
      } finally {
        setIsBulkActing(false);
        setPendingBulkAction(null);
      }
    },
    [handleKillProfile],
  );

  // Bulk run/stop only touch eligible profiles (run: not already running;
  // stop: currently running). An empty result shows a toast instead of a silent
  // no-op (guard), and 10+ targets require confirmation before launching/stopping.
  const handleBulkRun = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    const targets = profiles.filter(
      (p) => selectedProfiles.includes(p.id) && !runningProfiles.has(p.id),
    );
    if (targets.length === 0) {
      showErrorToast(t("profiles.bulkRun.noneToRun"));
      return;
    }
    if (targets.length >= 10) {
      setPendingBulkAction({ action: "run", profiles: targets });
      return;
    }
    void executeBulkRun(targets);
  }, [selectedProfiles, profiles, runningProfiles, executeBulkRun, t]);

  const handleBulkStop = useCallback(() => {
    if (selectedProfiles.length === 0) return;
    const targets = profiles.filter(
      (p) => selectedProfiles.includes(p.id) && runningProfiles.has(p.id),
    );
    if (targets.length === 0) {
      showErrorToast(t("profiles.bulkStop.noneToStop"));
      return;
    }
    if (targets.length >= 10) {
      setPendingBulkAction({ action: "stop", profiles: targets });
      return;
    }
    void executeBulkStop(targets);
  }, [selectedProfiles, profiles, runningProfiles, executeBulkStop, t]);

  const handleCopyCookiesToProfile = useCallback((profile: BrowserProfile) => {
    setSelectedProfilesForCookies([profile.id]);
    setCookieCopyDialogOpen(true);
  }, []);

  const handleOpenCookieManagement = useCallback((profile: BrowserProfile) => {
    setCurrentProfileForCookieManagement(profile);
    setCookieManagementDialogOpen(true);
  }, []);

  const handleGroupAssignmentComplete = useCallback(() => {
    // No need to manually reload - useProfileEvents will handle the update
    setGroupAssignmentDialogOpen(false);
    setSelectedProfilesForGroup([]);
  }, []);

  const handleProxyAssignmentComplete = useCallback(() => {
    // No need to manually reload - useProfileEvents will handle the update
    setProxyAssignmentDialogOpen(false);
    setSelectedProfilesForProxy([]);
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlistenStatus: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    const profilesWithTransfer = new Set<string>();
    void (async () => {
      try {
        unlistenStatus = await listen<{
          profile_id: string;
          status: string;
          error?: string;
          profile_name?: string;
        }>("profile-sync-status", (event) => {
          const { profile_id, status, error, profile_name } = event.payload;
          const toastId = `sync-${profile_id}`;
          const profile = profiles.find((p) => p.id === profile_id);
          const name =
            profile_name || profile?.name || t("common.labels.unknownProfile");

          if (status === "synced") {
            dismissToast(toastId);
            if (profilesWithTransfer.has(profile_id)) {
              profilesWithTransfer.delete(profile_id);
              showSuccessToast(t("sync.toast.profileSynced", { name }));
            }
          } else if (status === "error") {
            dismissToast(toastId);
            profilesWithTransfer.delete(profile_id);
            showErrorToast(
              error
                ? t("sync.toast.profileSyncFailedWithError", { name, error })
                : t("sync.toast.profileSyncFailed", { name }),
            );
          }
        });

        unlistenProgress = await listen<{
          profile_id: string;
          phase: string;
          total_files?: number;
          total_bytes?: number;
          completed_files?: number;
          completed_bytes?: number;
          speed_bytes_per_sec?: number;
          eta_seconds?: number;
          failed_count?: number;
          profile_name?: string;
        }>("profile-sync-progress", (event) => {
          const payload = event.payload;
          const toastId = `sync-${payload.profile_id}`;
          const profile = profiles.find((p) => p.id === payload.profile_id);
          const name =
            payload.profile_name ||
            profile?.name ||
            t("common.labels.unknownProfile");

          if (
            payload.phase === "started" ||
            payload.phase === "uploading" ||
            payload.phase === "downloading"
          ) {
            profilesWithTransfer.add(payload.profile_id);
            showSyncProgressToast(
              name,
              {
                completed_files: payload.completed_files ?? 0,
                total_files: payload.total_files ?? 0,
                completed_bytes: payload.completed_bytes ?? 0,
                total_bytes: payload.total_bytes ?? 0,
                speed_bytes_per_sec: payload.speed_bytes_per_sec ?? 0,
                eta_seconds: payload.eta_seconds ?? 0,
                failed_count: payload.failed_count ?? 0,
                phase: payload.phase,
              },
              { id: toastId, profileId: payload.profile_id },
            );
          }
        });
        // If the effect was torn down while we were awaiting the listeners,
        // unlisten immediately — the cleanup below already ran and would have
        // missed these handles. (Tauri unlisten is safe to call more than once.)
        if (disposed) {
          unlistenStatus?.();
          unlistenProgress?.();
        }
      } catch (error) {
        console.error("Failed to listen for sync events:", error);
      }
    })();
    return () => {
      disposed = true;
      if (unlistenStatus) unlistenStatus();
      if (unlistenProgress) unlistenProgress();
    };
  }, [profiles, t]);

  useEffect(() => {
    // Listen for URL open events. Guard against the effect tearing down (or
    // re-running) before the async listener setup resolves: if that happens,
    // run the cleanup as soon as it's available so the listeners never leak.
    let cleanup: (() => void) | undefined;
    let disposed = false;
    void listenForUrlEvents().then((cleanupFn) => {
      if (disposed) {
        cleanupFn?.();
        return;
      }
      cleanup = cleanupFn;
    });

    // Check for startup URLs (when app was launched as default browser)
    void checkCurrentUrl();

    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [listenForUrlEvents, checkCurrentUrl]);

  // E2E encryption listeners — surface password-required prompts and rollover
  // progress so the user isn't left guessing whether sealing finished.
  useEffect(() => {
    let disposed = false;
    let unlistenRequired: (() => void) | undefined;
    let unlistenStarted: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    let unlistenCompleted: (() => void) | undefined;
    let unlistenWayfernBlocked: (() => void) | undefined;

    void (async () => {
      unlistenRequired = await listen(
        "profile-sync-e2e-password-required",
        () => {
          showToast({
            id: "e2e-password-required",
            type: "error",
            title: t("encryption.required.title"),
            description: t("encryption.required.description"),
            duration: 12000,
            action: {
              label: t("encryption.required.openSettings"),
              onClick: () => {
                setSettingsDialogOpen(true);
                setCurrentPage("settings");
              },
            },
          });
        },
      );

      unlistenStarted = await listen("e2e-rollover-started", () => {
        showToast({
          id: "e2e-rollover",
          type: "loading",
          title: t("encryption.rollover.startedTitle"),
          description: t("encryption.rollover.startedDescription"),
          duration: Number.POSITIVE_INFINITY,
        });
      });

      unlistenProgress = await listen<{
        stage: string;
        done: number;
        total: number;
      }>("e2e-rollover-progress", (event) => {
        const { stage, done, total } = event.payload;
        showToast({
          id: "e2e-rollover",
          type: "loading",
          title: t("encryption.rollover.progressTitle", {
            stage: t(`encryption.rollover.stage.${stage}`),
          }),
          description: t("encryption.rollover.progressDescription", {
            done,
            total,
          }),
          duration: Number.POSITIVE_INFINITY,
        });
      });

      unlistenCompleted = await listen("e2e-rollover-completed", () => {
        showToast({
          id: "e2e-rollover",
          type: "success",
          title: t("encryption.rollover.completedTitle"),
          description: t("encryption.rollover.completedDescription"),
          duration: 5000,
        });
      });

      unlistenWayfernBlocked = await listen("wayfern-paid-blocked", () => {
        showToast({
          id: "wayfern-paid-blocked",
          type: "error",
          title: t("wayfernBlocked.title"),
          description: t("wayfernBlocked.description"),
          duration: 15000,
        });
      });

      // If the effect was torn down mid-setup, the cleanup below already ran
      // before these handles existed — unlisten them now so nothing leaks.
      if (disposed) {
        unlistenRequired?.();
        unlistenStarted?.();
        unlistenProgress?.();
        unlistenCompleted?.();
        unlistenWayfernBlocked?.();
      }
    })();

    return () => {
      disposed = true;
      unlistenRequired?.();
      unlistenStarted?.();
      unlistenProgress?.();
      unlistenCompleted?.();
      unlistenWayfernBlocked?.();
    };
  }, [t]);

  // Re-check Wayfern terms when a browser download completes
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    const setup = async () => {
      unlisten = await listen<{ stage: string }>(
        "download-progress",
        (event) => {
          if (event.payload.stage === "completed") {
            void checkTerms();
          }
        },
      );
    };
    void setup();
    return () => {
      if (unlisten) unlisten();
    };
  }, [checkTerms]);

  const profilesInGroup = useMemo(
    () =>
      !selectedGroupId || selectedGroupId === "__all__"
        ? profiles
        : profiles.filter((profile) => profile.group_id === selectedGroupId),
    [profiles, selectedGroupId],
  );

  const searchContext = useMemo<ProfileSearchContext>(
    () => ({
      groupNames: new Map(groupsData.map((g) => [g.id, g.name])),
      proxyNames: new Map(storedProxies.map((p) => [p.id, p.name || p.id])),
      vpnNames: new Map(),
      extensionGroupNames: new Map<string, string>(),
      runningProfiles,
    }),
    [groupsData, storedProxies, runningProfiles],
  );

  const clearTableSelection = useCallback(() => setSelectedProfiles([]), []);

  const profileFilters = useProfileTableFilters(
    profilesInGroup,
    searchQuery,
    searchContext,
    clearTableSelection,
  );

  const changeProfileSearch = useCallback((query: string) => {
    setSelectedProfiles([]);
    setSearchQuery(query);
  }, []);

  const clearProfileFilters = useCallback(() => {
    profileFilters.reset();
    setSearchQuery("");
  }, [profileFilters]);

  // Update loading states
  const isLoading = profilesLoading || groupsLoading || proxiesLoading;

  const subPageTitle =
    currentPage === "profiles"
      ? undefined
      : currentPage === "orgCenter"
        ? "Trung Tâm Quản Trị Tổ Chức (Org Center)"
        : currentPage === "teamManager"
          ? "Trung Tâm Điều Hành Trưởng Nhóm (Team Manager)"
          : currentPage === "auditLogs"
            ? "Nhật Ký Hoạt Động & Kiểm Toán An Toàn (Audit Trail)"
            : currentPage === "platformAdmin"
              ? "Bàn Điều Hành Quản Trị Nền Tảng (Super Admin)"
              : currentPage === "workspaces"
                ? "Quản Lý Không Gian Làm Việc (Workspace Center)"
                : t(`pageTitle.${currentPage}`);

  return (
    <div className="flex h-dvh overflow-hidden bg-background font-(family-name:--font-geist-sans)">
      <CloseConfirmDialog />
      <RailNav
        currentPage={currentPage}
        onNavigate={handleRailNavigate}
        onOpenAbout={() => {
          setAboutDialogOpen(true);
        }}
        onOpenAuditLogs={() => handleRailNavigate("auditLogs")}
        onOpenTeamManager={() => handleRailNavigate("teamManager")}
        onOpenPlatformAdmin={() => handleRailNavigate("platformAdmin")}
        onOpenOrgCenter={() => handleRailNavigate("orgCenter")}
        workspaces={workspaces}
        selectedWorkspaceId={selectedWorkspaceId}
        onWorkspaceSelect={handleWorkspaceSelect}
        onManageWorkspaces={() => handleRailNavigate("workspaces")}
        workspaceLoading={workspaceLoading || workspaceSwitching}
        totalProfilesCount={workspaceProfileCount(
          profiles.length,
          profilesReadyForWorkspace && !profilesLoading,
        )}
      />
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <HomeHeader
          pageTitle={subPageTitle}
          showPageContext={
            currentPage !== "workspaces" && currentPage !== "orgCenter"
          }
          updateInfo={appUpdates?.updateInfo}
          updateReady={appUpdates?.updateReady}
          isUpdating={appUpdates?.isUpdating}
          isCheckingForUpdates={
            appUpdates?.isCheckingForUpdates ?? isCheckingForUpdates
          }
          updateProgress={appUpdates?.updateProgress}
          onOpenUpdateDialog={() => appUpdates?.setIsUpdateDialogOpen(true)}
        />
        {appUpdates?.updateReady &&
          !appUpdates.bannerDismissed &&
          appUpdates.updateInfo && (
            <InlineUpdateBanner
              updateInfo={appUpdates.updateInfo}
              onRestart={appUpdates.handleRestart}
              onOpenDetails={() => appUpdates.setIsUpdateDialogOpen(true)}
              onDismiss={appUpdates.dismissBanner}
            />
          )}
        <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
          {currentPage === "profiles" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col px-3 pt-2.5"
            >
              <ProfilesDataTable
                workspaceId={selectedWorkspaceId}
                isLoading={
                  workspaceProfilesLoading ||
                  (isLoading && profiles.length === 0)
                }
                showOnboardingEmptyState={
                  profilesReadyForWorkspace &&
                  !workspaceProfilesFailed &&
                  profiles.length === 0
                }
                profiles={profileFilters.rows}
                filterControls={
                  <TableFilterControls
                    {...profileFilters}
                    query={searchQuery}
                    onQueryChange={changeProfileSearch}
                    showSearch={true}
                    count={profileFilters.rows.length}
                    total={profilesInGroup.length}
                    onReset={clearProfileFilters}
                  />
                }
                infoDialogProfile={
                  profileInfoDialog
                    ? (profiles.find(
                        (profile) => profile.id === profileInfoDialog.id,
                      ) ?? profileInfoDialog)
                    : null
                }
                onInfoDialogProfileChange={setProfileInfoDialog}
                onLaunchProfile={launchProfile}
                onKillProfile={handleKillProfile}
                onCloneProfile={handleCloneProfile}
                onShareProfile={
                  onShareProfiles && selectedWorkspaceId
                    ? (profile) =>
                        onShareProfiles([profile], selectedWorkspaceId)
                    : undefined
                }
                onSetPassword={handleSetPassword}
                onChangePassword={handleChangePassword}
                onRemovePassword={handleRemovePassword}
                onDeleteProfile={handleDeleteProfile}
                onRenameProfile={handleRenameProfile}
                onRefreshProfiles={refreshSelectedWorkspaceProfiles}
                isRefreshingProfiles={profilesRefreshing}
                onConfigureWayfern={handleConfigureWayfern}
                onCopyCookiesToProfile={handleCopyCookiesToProfile}
                onOpenCookieManagement={handleOpenCookieManagement}
                runningProfiles={runningProfiles}
                isUpdating={() => isUpdating}
                onDeleteSelectedProfiles={handleDeleteSelectedProfiles}
                onAssignProfilesToGroup={handleAssignProfilesToGroup}
                selectedGroupId={selectedGroupId}
                groups={groupsData}
                totalProfilesCount={profiles.length}
                onGroupSelect={handleSelectGroup}
                selectedProfiles={selectedProfiles}
                onSelectedProfilesChange={setSelectedProfiles}
                onBulkDelete={handleBulkDelete}
                onBulkGroupAssignment={handleBulkGroupAssignment}
                onBulkProxyAssignment={handleBulkProxyAssignment}
                onBulkCopyCookies={handleBulkCopyCookies}
                onBulkShare={
                  onShareProfiles && selectedWorkspaceId
                    ? () => {
                        const selected = new Set(selectedProfiles);
                        onShareProfiles(
                          profiles.filter((profile) =>
                            selected.has(profile.id),
                          ),
                          selectedWorkspaceId,
                        );
                      }
                    : undefined
                }
                onBulkRun={handleBulkRun}
                onBulkStop={handleBulkStop}
                bulkActionsUnlocked={automationUnlocked}
                onBulkExtensionGroupAssignment={
                  handleBulkExtensionGroupAssignment
                }
                onAssignExtensionGroup={handleAssignExtensionGroup}
                crossOsUnlocked={crossOsUnlocked}
                getProfileSyncInfo={getProfileSyncInfo}
                onLaunchWithSync={(profile) => {
                  setSyncLeaderProfile(profile);
                }}
                onCreateProfile={() => {
                  setCreateProfileDialogOpen(true);
                }}
                workspaceSelected={Boolean(selectedWorkspaceId)}
                onOpenGroupManagement={() => setCurrentPage("groups")}
                onOpenProxyManagement={() => {
                  setProxyManagementInitialTab("proxies");
                  setProxyManagementDialogOpen(true);
                }}
                onOpenMemberManagement={() => setCurrentPage("members")}
                workspaceName={activeWorkspace?.name}
                workspaceRole={activeWorkspace?.role}
                organizationName={activeWorkspace?.organizationName}
                maxProfiles={activeWorkspace?.maxProfiles}
              />
            </motion.div>
          )}

          {currentPage === "shortcuts" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col"
            >
              <ShortcutsPage groupTargets={orderedGroupTargets} />
            </motion.div>
          )}

          {settingsDialogOpen && (
            <SettingsDialog
              isOpen={settingsDialogOpen}
              onClose={() => {
                setSettingsDialogOpen(false);
                setCurrentPage("profiles");
              }}
              onIntegrationsOpen={() => {
                setSettingsDialogOpen(false);
                setIntegrationsDialogOpen(true);
                setCurrentPage("integrations");
              }}
              subPage={currentPage === "settings"}
            />
          )}

          {integrationsDialogOpen && (
            <IntegrationsDialog
              isOpen={integrationsDialogOpen}
              onClose={() => {
                setIntegrationsDialogOpen(false);
                setCurrentPage("profiles");
              }}
              subPage={currentPage === "integrations"}
              initialTab={integrationsInitialTab}
            />
          )}

          {proxyManagementDialogOpen && (
            <ProxyManagementDialog
              isOpen={proxyManagementDialogOpen}
              onClose={() => {
                setProxyManagementDialogOpen(false);
                setCurrentPage("profiles");
              }}
              subPage={currentPage === "proxies"}
              initialTab={proxyManagementInitialTab}
            />
          )}

          {extensionManagementDialogOpen && (
            <ExtensionManagementDialog
              isOpen={extensionManagementDialogOpen}
              workspaceId={selectedWorkspaceId}
              onClose={() => {
                setExtensionManagementDialogOpen(false);
                setCurrentPage("profiles");
              }}
              subPage={currentPage === "extensions"}
            />
          )}

          {currentPage === "account" && <PersonalAccount />}

          {currentPage === "members" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col overflow-hidden p-3"
            >
              {activeWorkspace?.organizationId ? (
                <WorkspaceMembers
                  organizationId={activeWorkspace.organizationId}
                  workspaceId={activeWorkspace.id}
                  onDataChanged={refreshWorkspaceList}
                />
              ) : (
                <p className="p-4 text-sm text-muted-foreground">
                  Chưa chọn workspace hợp lệ.
                </p>
              )}
            </motion.div>
          )}

          {currentPage === "groups" && activeWorkspace && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col overflow-hidden"
            >
              <ManagementResources
                workspaceId={activeWorkspace.id}
                kind="groups"
              />
            </motion.div>
          )}

          {currentPage === "orgCenter" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col overflow-hidden"
            >
              <OrganizationManagement onDataChanged={refreshWorkspaceList} />
            </motion.div>
          )}

          {currentPage === "teamManager" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col overflow-hidden"
            >
              <TeamManagerView
                onNavigateHome={() => setCurrentPage("profiles")}
                workspaceId={selectedWorkspaceId}
                workspaceName={
                  workspaces.find((w) => w.id === selectedWorkspaceId)?.name
                }
                profiles={profiles}
                groups={groupsData}
                onRefreshProfiles={refreshProfiles}
              />
            </motion.div>
          )}

          {currentPage === "auditLogs" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col overflow-hidden"
            >
              <AuditLogView
                onNavigateHome={() => setCurrentPage("profiles")}
                workspaceId={selectedWorkspaceId}
                workspaceName={
                  workspaces.find((w) => w.id === selectedWorkspaceId)?.name
                }
              />
            </motion.div>
          )}

          {currentPage === "platformAdmin" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col overflow-hidden"
            >
              <PlatformManagement
                onOpenAudit={() => handleRailNavigate("auditLogs")}
              />
            </motion.div>
          )}

          {currentPage === "accountManagement" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col overflow-hidden"
            >
              <AccountDirectory
                organizationId={
                  selectedWorkspaceId
                    ? undefined
                    : activeWorkspace?.organizationId
                }
                workspaceId={selectedWorkspaceId}
                onDataChanged={refreshWorkspaceList}
              />
            </motion.div>
          )}

          {currentPage === "workspaces" && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.25, ease: MOTION_EASE_OUT }}
              className="flex min-h-0 flex-1 flex-col overflow-hidden"
            >
              <OrganizationManagement
                onDataChanged={refreshWorkspaceList}
                platformView="workspaces"
              />
            </motion.div>
          )}
        </main>
      </div>

      <CreateProfileDialog
        isOpen={createProfileDialogOpen}
        workspaceId={selectedWorkspaceId}
        onClose={() => {
          setCreateProfileDialogOpen(false);
        }}
        onCreateProfile={handleCreateProfile}
        selectedGroupId={selectedGroupId}
      />

      <CommandPalette
        open={commandPaletteOpen}
        onOpenChange={setCommandPaletteOpen}
        onAction={runShortcut}
        groupTargets={orderedGroupTargets}
        onSelectGroup={(id) => {
          handleRailNavigate("profiles");
          handleSelectGroup(id);
        }}
        profiles={profiles}
        runningProfileIds={runningProfiles}
        onLaunchProfile={(profile) => {
          void launchProfile(profile);
        }}
        onKillProfile={(profile) => {
          void handleKillProfile(profile);
        }}
        onShowProfileInfo={(profile) => {
          handleRailNavigate("profiles");
          setProfileInfoDialog(profile);
        }}
        onCreateProfile={() => {
          setCreateProfileDialogOpen(true);
        }}
        onOpenAbout={() => {
          setAboutDialogOpen(true);
        }}
      />

      <AboutDialog
        isOpen={aboutDialogOpen}
        onCheckForUpdates={onCheckForUpdates}
        isCheckingForUpdates={isCheckingForUpdates}
        onClose={() => {
          setAboutDialogOpen(false);
        }}
      />

      <ConsistencyWarningDialog
        isOpen={consistencyWarning !== null}
        onClose={() => {
          setConsistencyWarning(null);
        }}
        profileName={consistencyWarning?.profile.name ?? ""}
        profileId={consistencyWarning?.profile.id ?? ""}
        result={consistencyWarning?.result ?? null}
      />

      {pendingUrls.map((pendingUrl) => (
        <ProfileSelectorDialog
          key={pendingUrl.id}
          isOpen={true}
          onClose={() => {
            setPendingUrls((prev) =>
              prev.filter((u) => u.id !== pendingUrl.id),
            );
          }}
          url={pendingUrl.url}
          isUpdating={() => isUpdating}
          runningProfiles={runningProfiles}
        />
      ))}

      <CloneProfileDialog
        isOpen={!!cloneProfile}
        onClose={() => {
          setCloneProfile(null);
        }}
        onCloneComplete={() => {
          void refreshProfiles();
        }}
        profile={cloneProfile}
      />

      <ProfilePasswordDialog
        isOpen={!!passwordDialogProfile}
        onClose={() => {
          pendingLaunchAfterUnlockRef.current = null;
          setPasswordDialogProfile(null);
        }}
        profile={passwordDialogProfile}
        mode={passwordDialogMode}
        onSuccess={(p) => {
          // Resume pending launch after unlock.
          if (
            passwordDialogMode === "unlock" &&
            pendingLaunchAfterUnlockRef.current?.id === p.id
          ) {
            const target = pendingLaunchAfterUnlockRef.current;
            pendingLaunchAfterUnlockRef.current = null;
            void launchProfile(target);
          }
        }}
      />

      <WayfernConfigDialog
        isOpen={wayfernConfigDialogOpen}
        onClose={() => {
          setWayfernConfigDialogOpen(false);
        }}
        profile={currentProfileForWayfernConfig}
        onSave={handleSaveWayfernConfig}
        isRunning={
          currentProfileForWayfernConfig
            ? runningProfiles.has(currentProfileForWayfernConfig.id)
            : false
        }
        crossOsUnlocked={crossOsUnlocked}
      />

      <GroupAssignmentDialog
        isOpen={groupAssignmentDialogOpen}
        onClose={() => {
          setGroupAssignmentDialogOpen(false);
        }}
        selectedProfiles={selectedProfilesForGroup}
        onAssignmentComplete={handleGroupAssignmentComplete}
      />

      {appUpdates && (
        <AppUpdateDialog
          isOpen={appUpdates.isUpdateDialogOpen}
          onClose={() => appUpdates.setIsUpdateDialogOpen(false)}
          updateInfo={appUpdates.updateInfo}
          isUpdating={appUpdates.isUpdating}
          updateProgress={appUpdates.updateProgress}
          updateReady={appUpdates.updateReady}
          onUpdate={() => void appUpdates.handleAppUpdate()}
          onRestart={appUpdates.handleRestart}
          autoUpdatesEnabled={appUpdates.autoUpdatesEnabled}
          onToggleAutoUpdates={appUpdates.setAutoUpdatesEnabled}
        />
      )}

      <ExtensionGroupAssignmentDialog
        isOpen={extensionGroupAssignmentDialogOpen}
        workspaceId={selectedWorkspaceId}
        onClose={() => {
          setExtensionGroupAssignmentDialogOpen(false);
        }}
        selectedProfiles={selectedProfilesForExtensionGroup}
        onAssignmentComplete={handleExtensionGroupAssignmentComplete}
        profiles={profiles}
      />

      <ProxyAssignmentDialog
        isOpen={proxyAssignmentDialogOpen}
        onClose={() => {
          setProxyAssignmentDialogOpen(false);
        }}
        selectedProfiles={selectedProfilesForProxy}
        onAssignmentComplete={handleProxyAssignmentComplete}
        profiles={profiles}
        storedProxies={storedProxies}
      />

      <CookieCopyDialog
        isOpen={cookieCopyDialogOpen}
        onClose={() => {
          setCookieCopyDialogOpen(false);
          setSelectedProfilesForCookies([]);
        }}
        selectedProfiles={selectedProfilesForCookies}
        profiles={profiles}
        runningProfiles={runningProfiles}
        onCopyComplete={() => {
          setSelectedProfilesForCookies([]);
        }}
      />

      <CookieManagementDialog
        isOpen={cookieManagementDialogOpen}
        onClose={() => {
          setCookieManagementDialogOpen(false);
          setCurrentProfileForCookieManagement(null);
        }}
        profile={currentProfileForCookieManagement}
      />

      <DeleteConfirmationDialog
        isOpen={pendingBulkAction !== null}
        onClose={() => {
          setPendingBulkAction(null);
        }}
        onConfirm={() => {
          if (!pendingBulkAction) return;
          if (pendingBulkAction.action === "run") {
            void executeBulkRun(pendingBulkAction.profiles);
          } else {
            void executeBulkStop(pendingBulkAction.profiles);
          }
        }}
        title={
          pendingBulkAction?.action === "stop"
            ? t("profiles.bulkStop.confirmTitle", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
            : t("profiles.bulkRun.confirmTitle", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
        }
        description={
          pendingBulkAction?.action === "stop"
            ? t("profiles.bulkStop.confirmDescription", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
            : t("profiles.bulkRun.confirmDescription", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
        }
        confirmButtonText={
          pendingBulkAction?.action === "stop"
            ? t("profiles.bulkStop.confirmButton", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
            : t("profiles.bulkRun.confirmButton", {
                count: pendingBulkAction?.profiles.length ?? 0,
              })
        }
        confirmButtonVariant="default"
        isLoading={isBulkActing}
      />
      <DeleteConfirmationDialog
        isOpen={showBulkDeleteConfirmation}
        onClose={() => {
          setShowBulkDeleteConfirmation(false);
        }}
        onConfirm={confirmBulkDelete}
        title={t("profiles.bulkDelete.title")}
        description={t("profiles.bulkDelete.description", {
          count: selectedProfiles.length,
        })}
        confirmButtonText={t("profiles.bulkDelete.confirmButton", {
          count: selectedProfiles.length,
        })}
        isLoading={isBulkDeleting}
        profileIds={selectedProfiles}
        profiles={profiles.map((p) => ({ id: p.id, name: p.name }))}
      />

      {/* Wayfern Terms and Conditions Dialog - shown if terms not accepted */}
      <WayfernTermsDialog
        isOpen={!termsLoading && termsAccepted === false}
        onAccepted={checkTerms}
      />

      <SyncFollowerDialog
        isOpen={syncLeaderProfile !== null}
        onClose={() => {
          setSyncLeaderProfile(null);
        }}
        leaderProfile={syncLeaderProfile}
        allProfiles={profiles}
        runningProfiles={runningProfiles}
      />

      <PendingInvitesDialog
        isOpen={pendingInvitesOpen}
        initialCode={pendingInviteLink}
        onClose={() => {
          setPendingInvitesOpen(false);
          setPendingInviteLink("");
        }}
      />
    </div>
  );
}
