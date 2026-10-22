"use client";

import * as CheckboxPrimitive from "@radix-ui/react-checkbox";
import { invoke } from "@tauri-apps/api/core";
import { motion, useReducedMotion } from "motion/react";
import type * as React from "react";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCheck,
  LuChevronDown,
  LuChevronRight,
  LuCpu,
  LuDownload,
  LuFlame,
  LuFolder,
  LuGlobe,
  LuInfo,
  LuLayers,
  LuLoader,
  LuLock,
  LuLockOpen,
  LuMonitor,
  LuShieldCheck,
  LuShuffle,
  LuTriangleAlert,
} from "react-icons/lu";
import { CreateProfileProxyDialog } from "@/components/create-profile-proxy-dialog";
import { LoadingButton } from "@/components/loading-button";
import { ProfileGlyph } from "@/components/profile-glyph";
import type { ProxySettingsDraft } from "@/components/proxy-form-dialog";
import {
  AnimatedDisclosureChevron,
  AnimatedDisclosureContent,
} from "@/components/ui/animated-disclosure";
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
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { SoftFields } from "@/components/ui/field-variant";
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
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { WayfernConfigForm } from "@/components/wayfern-config-form";
import { useBrowserDownload } from "@/hooks/use-browser-download";
import { useInputModality } from "@/hooks/use-input-modality";
import { useProxyEvents } from "@/hooks/use-proxy-events";
import { parseBackendError } from "@/lib/backend-errors";
import { getOSDisplayName, getOSIcon } from "@/lib/browser-utils";
import { DNS_BLOCKLIST_LEVELS } from "@/lib/dns-blocklist-levels";
import { MOTION_EASE_OUT } from "@/lib/motion";
import { showErrorToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import type { WayfernConfig, WayfernOS } from "@/types";

const PASSWORD_MIN_LEN = 8;
type BrowserKernel = "wayfern";

const OS_OPTIONS: WayfernOS[] = ["windows", "macos", "linux", "android", "ios"];

const COMMON_RESOLUTIONS = [
  { width: 1920, height: 1080 },
  { width: 2560, height: 1440 },
  { width: 1440, height: 900 },
  { width: 1536, height: 864 },
  { width: 1680, height: 1050 },
  { width: 1366, height: 768 },
];

const COMMON_CORES = [8, 12, 16, 24];
const COMMON_MEMORY = [8, 16, 32];

const getCurrentOS = (): WayfernOS => {
  if (typeof navigator === "undefined") return "linux";
  const platform = navigator.platform.toLowerCase();
  if (platform.includes("win")) return "windows";
  if (platform.includes("mac")) return "macos";
  return "linux";
};

const defaultWayfernConfig = (): WayfernConfig => ({
  os: getCurrentOS(),
});

interface CreateProfileDialogProps {
  isOpen: boolean;
  onClose: () => void;
  workspaceId: string | null;
  onCreateProfile: (profileData: {
    name: string;
    idempotencyKey: string;
    browserStr: BrowserKernel;
    version?: string;
    releaseType: string;
    proxyId?: string;
    proxySettingsDraft?: ProxySettingsDraft;
    vpnId?: string;
    wayfernConfig?: WayfernConfig;
    groupId?: string;
    extensionGroupId?: string;
    ephemeral?: boolean;
    dnsBlocklist?: string;
    launchHook?: string;
    password?: string;
  }) => Promise<void>;
  selectedGroupId?: string;
}

export function CreateProfileDialog({
  isOpen,
  onClose,
  workspaceId,
  onCreateProfile,
  selectedGroupId,
}: CreateProfileDialogProps) {
  const { t } = useTranslation();
  const osListboxId = useId();
  const osLabelId = useId();
  const osValueId = useId();
  const advancedOptionsId = useId();
  const reduceMotion = useReducedMotion() ?? false;
  const inputModality = useInputModality();

  const [profileName, setProfileName] = useState("");
  const [groupId, setGroupId] = useState<string>(
    selectedGroupId && selectedGroupId !== "__all__" ? selectedGroupId : "none",
  );
  const [groupPopoverOpen, setGroupPopoverOpen] = useState(false);
  const [profileGroups, setProfileGroups] = useState<
    { id: string; name: string }[]
  >([]);

  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [selectedProxyId, setSelectedProxyId] = useState<string>();
  const [proxySettingsDraft, setProxySettingsDraft] =
    useState<ProxySettingsDraft>();

  const handleProxySelect = useCallback((proxyId?: string) => {
    setSelectedProxyId(proxyId);
    setProxySettingsDraft(undefined);
  }, []);

  const handleProxyDraftChange = useCallback(
    (settings?: ProxySettingsDraft) => {
      setProxySettingsDraft(settings);
      if (settings) setSelectedProxyId(undefined);
    },
    [],
  );

  const [osPopoverOpen, setOsPopoverOpen] = useState(false);
  const [dnsBlocklist, setDnsBlocklist] = useState("");
  const [launchHook, setLaunchHook] = useState("");
  const [hardwareCores, setHardwareCores] = useState<number>(16);
  const [hardwareMemory, setHardwareMemory] = useState<number>(16);
  const [wayfernConfig, setWayfernConfig] =
    useState<WayfernConfig>(defaultWayfernConfig);
  const [isCreating, setIsCreating] = useState(false);
  const [ephemeral, setEphemeral] = useState(false);
  const pendingCreateIdempotencyRef = useRef<{
    fingerprint: string;
    key: string;
  } | null>(null);

  const [enablePassword, setEnablePassword] = useState(false);
  const [password, setPassword] = useState("");
  const [passwordConfirm, setPasswordConfirm] = useState("");
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [selectedExtensionGroupId, setSelectedExtensionGroupId] =
    useState<string>();
  const [extensionGroups, setExtensionGroups] = useState<
    { id: string; name: string; extension_ids: string[] }[]
  >([]);
  const [availableVersions, setAvailableVersions] = useState<string[]>([]);
  const [catalogVersions, setCatalogVersions] = useState<string[]>([]);
  const [selectedVersion, setSelectedVersion] = useState<string>();
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [catalogDetailsOpen, setCatalogDetailsOpen] = useState(false);
  const versionRequestRef = useRef(0);
  const { storedProxies } = useProxyEvents();
  const {
    isBrowserDownloading,
    downloadBrowser,
    loadVersions,
    loadDownloadedVersions,
    downloadedVersionsMap,
    downloadProgress,
  } = useBrowserDownload();

  const loadReleaseTypes = useCallback(
    async (isUserRetry = false) => {
      const request = ++versionRequestRef.current;
      let catalogVersions: string[] = [];
      try {
        const releases = await loadVersions("wayfern");
        catalogVersions = releases.map((release) => release.tag_name);
        if (versionRequestRef.current === request) {
          setCatalogVersions(catalogVersions);
          setCatalogError(null);
        }
      } catch (error) {
        console.warn("Failed to load verified releases:", error);
        const backendError = parseBackendError(error);
        const errorKey =
          backendError?.code.toLowerCase() ?? "api_request_failed";
        const errorDescription = t(`createProfile.version.errors.${errorKey}`, {
          defaultValue:
            "The BugLogin API does not currently provide a verified browser catalog. Profile settings can still be saved; browser install and launch need a published catalog.",
        });

        if (versionRequestRef.current === request) {
          setCatalogVersions([]);
          setCatalogError(errorDescription);
        }
        if (isUserRetry) {
          showErrorToast(t("createProfile.version.browserLabel"), {
            description: errorDescription,
          });
        }
      }

      let installedVersions: string[] = [];
      try {
        installedVersions = await loadDownloadedVersions("wayfern");
      } catch (error) {
        console.warn("Failed to load installed Wayfern versions:", error);
      }

      if (versionRequestRef.current === request) {
        const versions = [
          ...new Set([...catalogVersions, ...installedVersions]),
        ].sort((left, right) =>
          right.localeCompare(left, undefined, { numeric: true }),
        );
        setAvailableVersions(versions);
        const preferred = versions[0];
        setSelectedVersion((current) =>
          current && versions.includes(current) ? current : preferred,
        );
      }
    },
    [loadDownloadedVersions, loadVersions, t],
  );

  useEffect(() => {
    if (!isOpen) return;
    void loadReleaseTypes();

    void invoke<{ id: string; name: string }[]>("get_profile_groups")
      .then(setProfileGroups)
      .catch(() => setProfileGroups([]));

    if (!workspaceId) {
      setExtensionGroups([]);
    } else {
      void invoke<{ id: string; name: string; extension_ids: string[] }[]>(
        "list_extension_groups",
        { workspaceId },
      )
        .then(setExtensionGroups)
        .catch((error) => {
          console.error("Failed to load extension groups:", error);
          setExtensionGroups([]);
        });
    }

    return () => {
      versionRequestRef.current += 1;
    };
  }, [isOpen, workspaceId, loadReleaseTypes]);

  useEffect(() => {
    if (selectedGroupId && selectedGroupId !== "__all__") {
      setGroupId(selectedGroupId);
    } else {
      setGroupId("none");
    }
  }, [selectedGroupId]);

  const downloadedVersions = downloadedVersionsMap.wayfern ?? [];
  const creatableVersion =
    selectedVersion || downloadedVersions[0] || availableVersions[0];
  const browserVersionStatus =
    creatableVersion && downloadedVersions.includes(creatableVersion)
      ? t("createProfile.version.installed")
      : catalogError
        ? t("createProfile.version.fetchError")
        : availableVersions.length > 0
          ? t("createProfile.version.available", {
              browser: t("createProfile.antiDetect.chromium"),
              version: creatableVersion,
            })
          : t("createProfile.version.fetching");

  const downloading = isBrowserDownloading("wayfern");
  const canDownloadSelectedVersion =
    !!creatableVersion &&
    catalogVersions.includes(creatableVersion) &&
    !downloadedVersions.includes(creatableVersion);
  const isCreateDisabled = !profileName.trim() || downloading || isCreating;
  const hostOs = getCurrentOS();
  const selectedOs = wayfernConfig.os ?? hostOs;
  const crossOs = selectedOs !== hostOs;
  const SelectedOsIcon = getOSIcon(selectedOs);
  const selectedGroupName =
    groupId && groupId !== "none"
      ? (profileGroups.find((g) => g.id === groupId)?.name ??
        t("createProfile.noGroup"))
      : t("createProfile.noGroup");

  const clearPassword = () => {
    setEnablePassword(false);
    setPassword("");
    setPasswordConfirm("");
    setPasswordError(null);
  };

  const handleDownloadSelectedVersion = async () => {
    if (!canDownloadSelectedVersion || !creatableVersion) return;
    try {
      await downloadBrowser("wayfern", creatableVersion);
    } catch (error) {
      // The download hook reports the verified installer failure to the user.
      console.error("Failed to install selected Wayfern version:", error);
    }
  };

  const handleClose = () => {
    versionRequestRef.current += 1;
    pendingCreateIdempotencyRef.current = null;
    setProfileName("");
    setAdvancedOpen(false);
    setSelectedProxyId(undefined);
    setProxySettingsDraft(undefined);
    setOsPopoverOpen(false);
    setDnsBlocklist("");
    setLaunchHook("");
    setSelectedExtensionGroupId(undefined);
    setAvailableVersions([]);
    setCatalogVersions([]);
    setSelectedVersion(undefined);
    setCatalogError(null);
    setWayfernConfig(defaultWayfernConfig());
    setEphemeral(false);
    clearPassword();
    onClose();
  };

  const handleCreate = async () => {
    if (isCreateDisabled) return;
    if (enablePassword && !ephemeral) {
      if (password.length < PASSWORD_MIN_LEN) {
        setPasswordError(
          t("profilePassword.errors.tooShort", { min: PASSWORD_MIN_LEN }),
        );
        return;
      }
      if (password !== passwordConfirm) {
        setPasswordError(t("profilePassword.errors.mismatch"));
        return;
      }
    }
    setPasswordError(null);
    setIsCreating(true);

    const actualGroupId = groupId && groupId !== "none" ? groupId : undefined;

    const payloadFingerprint = JSON.stringify({
      name: profileName.trim(),
      version: creatableVersion,
      proxyId: selectedProxyId,
      proxySettingsDraft,
      wayfernConfig,
      groupId: actualGroupId,
      extensionGroupId: selectedExtensionGroupId,
      ephemeral,
      dnsBlocklist,
      launchHook: launchHook.trim(),
      password: enablePassword && !ephemeral ? password : undefined,
    });
    const pendingCreate = pendingCreateIdempotencyRef.current;
    const idempotencyKey =
      pendingCreate?.fingerprint === payloadFingerprint
        ? pendingCreate.key
        : window.crypto.randomUUID();
    pendingCreateIdempotencyRef.current = {
      fingerprint: payloadFingerprint,
      key: idempotencyKey,
    };
    try {
      await onCreateProfile({
        name: profileName.trim(),
        idempotencyKey,
        browserStr: "wayfern",
        version: creatableVersion,
        releaseType: "stable",
        proxyId: selectedProxyId,
        proxySettingsDraft,
        wayfernConfig,
        groupId: actualGroupId,
        extensionGroupId: selectedExtensionGroupId,
        ephemeral,
        dnsBlocklist: dnsBlocklist || undefined,
        launchHook: launchHook.trim() || undefined,
        password: enablePassword && !ephemeral ? password : undefined,
      });
      pendingCreateIdempotencyRef.current = null;
      handleClose();
    } catch (error) {
      console.error("Failed to create profile:", error);
    } finally {
      setIsCreating(false);
    }
  };

  const updateWayfernConfig = useCallback(
    (key: keyof WayfernConfig, value: unknown) => {
      setWayfernConfig((previous) => ({ ...previous, [key]: value }));
    },
    [],
  );

  const handleRandomizeFingerprint = useCallback(() => {
    const res =
      COMMON_RESOLUTIONS[Math.floor(Math.random() * COMMON_RESOLUTIONS.length)];
    const cores = COMMON_CORES[Math.floor(Math.random() * COMMON_CORES.length)];
    const memory =
      COMMON_MEMORY[Math.floor(Math.random() * COMMON_MEMORY.length)];

    setHardwareCores(cores);
    setHardwareMemory(memory);

    setWayfernConfig((prev) => ({
      ...prev,
      screen_max_width: res.width,
      screen_max_height: res.height,
      screen_min_width: res.width,
      screen_min_height: res.height,
      randomize_fingerprint_on_launch: true,
    }));
  }, []);

  const animateEntry = !reduceMotion && inputModality === "pointer";
  const rise = (delay: number) =>
    animateEntry
      ? {
          initial: { y: 4 },
          animate: { y: 0 },
          transition: { ...ENTRY_SPRING, delay },
        }
      : {};

  const reveal = animateEntry
    ? {
        initial: { y: -4, scale: 0.99 },
        animate: { y: 0, scale: 1 },
        transition: ENTRY_SPRING,
      }
    : {};

  // Formatted Proxy Information for Live Summary Card
  const activeProxyName = selectedProxyId
    ? (storedProxies.find((p) => p.id === selectedProxyId)?.name ??
      t("createProfile.fingerprintSummary.autoFromProxy"))
    : proxySettingsDraft
      ? `${proxySettingsDraft.host}:${proxySettingsDraft.port} (${proxySettingsDraft.proxy_type})`
      : t("createProfile.fingerprintSummary.direct");

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open && !isCreating) handleClose();
      }}
    >
      <DialogContent
        className="flex max-h-[calc(100dvh-max(1rem,5vh)-1rem)] w-[calc(100vw-2rem)] max-w-5xl flex-col gap-3 rounded-2xl border-0 p-5 shadow-2xl ring-1 ring-foreground/10"
        style={
          {
            top: "max(1rem, 5vh)",
            "--tw-translate-y": "0px",
          } as React.CSSProperties
        }
        aria-describedby={undefined}
        dismissible={!isCreating}
      >
        {/* Header */}
        <DialogHeader className="shrink-0 flex flex-row items-center justify-between pb-2 border-b border-border/50">
          <div className="flex items-center gap-2">
            <div className="flex size-7 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <LuGlobe className="size-4" />
            </div>
            <div>
              <DialogTitle className="text-base font-semibold">
                {t("createProfile.title")}
              </DialogTitle>
            </div>
            <Badge
              variant="soft"
              className="text-[10px] font-mono uppercase tracking-wider py-0 px-1.5 ml-1"
            >
              Antidetect
            </Badge>
          </div>
        </DialogHeader>

        {/* Top Control Bar: Compact 1-2 rows */}
        <div className="flex flex-col gap-2 shrink-0">
          <div className="grid grid-cols-1 sm:grid-cols-[1fr_210px] gap-2 items-center">
            {/* Profile Name Field */}
            <motion.div
              data-slot="profile-name-field"
              className="flex items-center gap-2 rounded-lg bg-foreground/4 px-2.5 py-1.5 transition-colors focus-within:bg-foreground/6 focus-within:ring-2 focus-within:ring-foreground/10 hover:bg-foreground/5"
              {...rise(0)}
            >
              <label
                htmlFor="profile-name"
                className="flex min-w-0 flex-1 cursor-text items-center gap-2"
              >
                <ProfileGlyph
                  seed={profileName}
                  routed={!!selectedProxyId || !!proxySettingsDraft}
                  locked={enablePassword}
                  ephemeral={ephemeral}
                  busy={isCreating}
                />
                <span className="sr-only">
                  {t("createProfile.profileName")}
                </span>
                <Input
                  id="profile-name"
                  variant="bare"
                  autoFocus
                  autoComplete="off"
                  spellCheck={false}
                  value={profileName}
                  onChange={(event) => setProfileName(event.target.value)}
                  onKeyDown={(event) => {
                    if (
                      event.key === "Enter" &&
                      !event.nativeEvent.isComposing
                    ) {
                      event.preventDefault();
                      void handleCreate();
                    }
                  }}
                  placeholder={t("createProfile.profileNamePlaceholder")}
                  disabled={isCreating}
                  className="h-8 text-sm font-medium md:text-sm"
                />
              </label>
            </motion.div>

            {/* Folder / Group Selector (Filterable Combobox) */}
            <div className="min-w-0">
              <Popover
                open={groupPopoverOpen}
                onOpenChange={setGroupPopoverOpen}
              >
                <PopoverTrigger asChild>
                  <Button
                    variant="soft"
                    role="combobox"
                    aria-expanded={groupPopoverOpen}
                    disabled={isCreating}
                    className="h-[38px] w-full justify-between rounded-lg px-2.5 text-xs font-normal"
                  >
                    <div className="flex min-w-0 items-center gap-1.5 truncate">
                      <LuFolder className="size-3.5 shrink-0 text-muted-foreground" />
                      <span className="truncate text-foreground font-medium">
                        {selectedGroupName}
                      </span>
                    </div>
                    <LuChevronDown className="size-3.5 shrink-0 text-muted-foreground opacity-60" />
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-60 p-0" align="end" sideOffset={4}>
                  <Command>
                    <CommandInput
                      placeholder={t("createProfile.searchGroup", {
                        defaultValue: "Search group...",
                      })}
                      className="h-8 text-xs"
                    />
                    <CommandList className="max-h-48">
                      <CommandEmpty className="py-2.5 text-center text-xs text-muted-foreground">
                        {t("createProfile.groupNotFound", {
                          defaultValue: "No group found",
                        })}
                      </CommandEmpty>
                      <CommandGroup>
                        <CommandItem
                          value="none no-group default Không có nhóm"
                          onSelect={() => {
                            setGroupId("none");
                            setGroupPopoverOpen(false);
                          }}
                          className="flex cursor-pointer items-center justify-between gap-2 py-1.5 text-xs"
                        >
                          <div className="flex min-w-0 items-center gap-2">
                            <LuFolder className="size-3.5 text-muted-foreground" />
                            <span className="truncate">
                              {t("createProfile.noGroup")}
                            </span>
                          </div>
                          {(!groupId || groupId === "none") && (
                            <LuCheck className="size-3.5 text-primary shrink-0" />
                          )}
                        </CommandItem>
                        {profileGroups.map((group) => (
                          <CommandItem
                            key={group.id}
                            value={group.name}
                            onSelect={() => {
                              setGroupId(group.id);
                              setGroupPopoverOpen(false);
                            }}
                            className="flex cursor-pointer items-center justify-between gap-2 py-1.5 text-xs"
                          >
                            <div className="flex min-w-0 items-center gap-2">
                              <LuFolder className="size-3.5 text-muted-foreground" />
                              <span className="truncate font-medium">
                                {group.name}
                              </span>
                            </div>
                            {groupId === group.id && (
                              <LuCheck className="size-3.5 text-primary shrink-0" />
                            )}
                          </CommandItem>
                        ))}
                      </CommandGroup>
                    </CommandList>
                  </Command>
                </PopoverContent>
              </Popover>
            </div>
          </div>

          {/* Quick Pill Controls */}
          <div className="flex flex-wrap items-center gap-1.5">
            <span id={osLabelId} className="sr-only">
              {t("fingerprint.osLabel")}
            </span>
            <Popover open={osPopoverOpen} onOpenChange={setOsPopoverOpen}>
              <PopoverTrigger asChild>
                <button
                  id="profile-os"
                  type="button"
                  role="combobox"
                  aria-expanded={osPopoverOpen}
                  aria-controls={osListboxId}
                  aria-labelledby={`${osLabelId} ${osValueId}`}
                  className={cn(
                    optionPill(crossOs),
                    "shrink-0 h-7 text-xs px-2.5",
                  )}
                  disabled={isCreating}
                >
                  <PillIcon on={crossOs} animate={animateEntry}>
                    <SelectedOsIcon />
                  </PillIcon>
                  <PillLabel
                    id={osValueId}
                    label={getOSDisplayName(selectedOs)}
                    animate={animateEntry}
                  />
                  <LuChevronDown
                    className={cn(
                      "-mr-0.5 opacity-60 transition-transform duration-200 motion-reduce:transition-none",
                      osPopoverOpen && "rotate-180",
                    )}
                    aria-hidden="true"
                  />
                </button>
              </PopoverTrigger>
              <PopoverContent className="w-48 p-0" align="start" sideOffset={6}>
                <Command>
                  <CommandList id={osListboxId}>
                    <CommandGroup>
                      {OS_OPTIONS.map((os) => {
                        const mobileComingSoon =
                          os === "android" || os === "ios";
                        const Icon = getOSIcon(os);
                        return (
                          <CommandItem
                            key={os}
                            value={os}
                            disabled={mobileComingSoon}
                            onSelect={() => {
                              if (mobileComingSoon) return;
                              updateWayfernConfig("os", os);
                              setOsPopoverOpen(false);
                            }}
                          >
                            <LuCheck
                              className={cn(
                                "size-4 shrink-0",
                                selectedOs === os ? "opacity-100" : "opacity-0",
                              )}
                              aria-hidden="true"
                            />
                            <Icon
                              className="size-3.5 shrink-0"
                              aria-hidden="true"
                            />
                            <span className="truncate">
                              {getOSDisplayName(os)}
                            </span>
                          </CommandItem>
                        );
                      })}
                    </CommandGroup>
                  </CommandList>
                </Command>
              </PopoverContent>
            </Popover>

            <OptionToggle
              id="enable-password"
              checked={enablePassword}
              onCheckedChange={(checked) => {
                if (checked) {
                  setEnablePassword(true);
                  setEphemeral(false);
                } else clearPassword();
              }}
              disabled={isCreating}
              animate={animateEntry}
              icon={enablePassword ? <LuLock /> : <LuLockOpen />}
              label={t("profiles.passwordProtectedBadge")}
              hint={t("createProfile.passwordProtect.description")}
            />

            <OptionToggle
              id="ephemeral"
              checked={ephemeral}
              onCheckedChange={(checked) => {
                setEphemeral(checked);
                if (checked) clearPassword();
              }}
              disabled={isCreating}
              animate={animateEntry}
              icon={<LuFlame className={cn(ephemeral && "fill-current")} />}
              label={t("profiles.ephemeral")}
              hint={t("profiles.ephemeralDescription")}
            />
          </div>
        </div>

        {/* Master-Detail 2-Column Body: Fully scrollable if content overflows (no clip/cut off) */}
        <div className="grid grid-cols-1 md:grid-cols-[1fr_310px] gap-4 min-h-0 flex-1 overflow-y-auto pr-1">
          {/* Left Column: Configuration Forms */}
          <div
            data-slot="profile-create-fields"
            className="flex flex-col gap-3 min-h-0 pb-2"
          >
            {/* Password input row if enabled */}
            {enablePassword && (
              <motion.div
                className="space-y-1.5 rounded-lg border border-border/50 bg-card/40 p-2.5"
                {...reveal}
              >
                <div className="grid gap-2 sm:grid-cols-2">
                  <div>
                    <Label
                      htmlFor="profile-password"
                      className="text-xs text-muted-foreground mb-1 block"
                    >
                      {t("profilePassword.fields.newPassword")}
                    </Label>
                    <Input
                      id="profile-password"
                      variant="soft"
                      type="password"
                      value={password}
                      onChange={(event) => {
                        setPassword(event.target.value);
                        setPasswordError(null);
                      }}
                      placeholder={t("profilePassword.fields.newPassword")}
                      autoComplete="new-password"
                      aria-invalid={!!passwordError}
                      aria-describedby={
                        passwordError ? "profile-password-error" : undefined
                      }
                      disabled={isCreating}
                      className="h-8 text-xs"
                    />
                  </div>
                  <div>
                    <Label
                      htmlFor="profile-password-confirm"
                      className="text-xs text-muted-foreground mb-1 block"
                    >
                      {t("profilePassword.fields.confirm")}
                    </Label>
                    <Input
                      id="profile-password-confirm"
                      variant="soft"
                      type="password"
                      value={passwordConfirm}
                      onChange={(event) => {
                        setPasswordConfirm(event.target.value);
                        setPasswordError(null);
                      }}
                      placeholder={t("profilePassword.fields.confirm")}
                      autoComplete="new-password"
                      aria-invalid={!!passwordError}
                      aria-describedby={
                        passwordError ? "profile-password-error" : undefined
                      }
                      disabled={isCreating}
                      className="h-8 text-xs"
                    />
                  </div>
                </div>
                {passwordError && (
                  <motion.p
                    key={passwordError}
                    id="profile-password-error"
                    role="alert"
                    className="px-1 text-xs text-destructive-text"
                    initial={animateEntry ? { x: 0 } : false}
                    animate={animateEntry ? { x: [0, -4, 4, -2, 0] } : { x: 0 }}
                    transition={{ duration: 0.32, ease: MOTION_EASE_OUT }}
                  >
                    {passwordError}
                  </motion.p>
                )}
              </motion.div>
            )}

            {crossOs && (
              <motion.p
                role="status"
                className={cn(STATUS_CLASS, "items-start py-1.5 text-xs")}
                {...reveal}
              >
                <LuTriangleAlert
                  className="mt-0.5 size-3.5 shrink-0 text-warning"
                  aria-hidden="true"
                />
                {t("fingerprint.crossOsWarning")}
              </motion.p>
            )}

            {/* Section 1: Browser Core (Generic, user-friendly with clean layout & live progress) */}
            <div className="rounded-lg border border-border/50 bg-card/40 p-3 space-y-2.5">
              <div className="flex items-center justify-between gap-3 text-xs">
                <div className="flex items-center gap-2.5 min-w-0">
                  <div className="flex size-7 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary border border-primary/20">
                    <LuGlobe className="size-4" />
                  </div>
                  <div className="min-w-0">
                    <div className="flex items-center gap-1.5 font-medium text-foreground">
                      <span>Browser</span>
                      <span className="text-[11px] text-muted-foreground font-normal">
                        ({t("createProfile.version.browserLabel")})
                      </span>
                    </div>
                  </div>

                  {catalogError && (
                    <Popover
                      open={catalogDetailsOpen}
                      onOpenChange={setCatalogDetailsOpen}
                    >
                      <Tooltip delayDuration={200}>
                        <TooltipTrigger asChild>
                          <PopoverTrigger asChild>
                            <button
                              type="button"
                              className="inline-flex size-4 items-center justify-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground transition-colors cursor-pointer"
                              aria-label="Browser catalog status details"
                              onClick={() => {
                                showErrorToast(
                                  t("createProfile.version.browserLabel"),
                                  {
                                    description: catalogError,
                                  },
                                );
                              }}
                            >
                              <LuInfo className="size-3.5 text-warning/80 hover:text-warning transition-colors" />
                            </button>
                          </PopoverTrigger>
                        </TooltipTrigger>
                        <TooltipContent
                          side="top"
                          sideOffset={4}
                          className="text-[11px] max-w-64"
                        >
                          Click or hover for catalog notice
                        </TooltipContent>
                      </Tooltip>
                      <PopoverContent
                        side="top"
                        align="start"
                        className="w-80 p-3 text-xs space-y-2"
                      >
                        <div className="flex items-center gap-1.5 font-semibold text-foreground">
                          <LuInfo className="size-4 text-warning shrink-0" />
                          <span>Catalog Notice</span>
                        </div>
                        <p className="text-muted-foreground leading-relaxed text-[11px]">
                          {catalogError}
                        </p>
                        <div className="flex justify-end pt-1">
                          <Button
                            size="sm"
                            variant="soft"
                            className="h-7 text-xs px-2.5"
                            onClick={() => void loadReleaseTypes(true)}
                          >
                            {t("common.buttons.retry")}
                          </Button>
                        </div>
                      </PopoverContent>
                    </Popover>
                  )}
                </div>

                <div className="flex items-center gap-2 shrink-0">
                  <Select
                    value={creatableVersion ?? ""}
                    onValueChange={setSelectedVersion}
                    disabled={
                      availableVersions.length === 0 ||
                      downloading ||
                      isCreating
                    }
                  >
                    <SelectTrigger
                      aria-label={t("createProfile.version.browserLabel")}
                      className="h-8 w-44 text-xs font-mono"
                    >
                      <SelectValue
                        placeholder={
                          catalogError
                            ? t("createProfile.version.fetchError")
                            : t("createProfile.version.fetching")
                        }
                      />
                    </SelectTrigger>
                    <SelectContent>
                      {availableVersions.map((version) => (
                        <SelectItem
                          key={version}
                          value={version}
                          className="text-xs font-mono"
                        >
                          {version}
                          {downloadedVersions.includes(version)
                            ? ` · ${t("createProfile.version.installed")}`
                            : ""}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>

                  {/* Status Indicator / Actions */}
                  {creatableVersion &&
                  downloadedVersions.includes(creatableVersion) ? (
                    <span className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[11px] font-medium bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20 shrink-0">
                      <LuCheck className="size-3.5" />
                      <span>{t("createProfile.version.installed")}</span>
                    </span>
                  ) : downloading ? (
                    <span className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[11px] font-medium bg-primary/10 text-primary border border-primary/20 shrink-0">
                      <LuLoader className="size-3.5 animate-spin" />
                      <span>
                        {downloadProgress?.percentage
                          ? `${downloadProgress.percentage}%`
                          : t("common.buttons.downloading")}
                      </span>
                    </span>
                  ) : canDownloadSelectedVersion ? (
                    <Button
                      type="button"
                      size="sm"
                      variant="soft"
                      className="h-8 shrink-0 text-xs gap-1.5 px-3"
                      onClick={() => void handleDownloadSelectedVersion()}
                      disabled={downloading || isCreating}
                    >
                      <LuDownload className="size-3.5" />
                      <span>{t("common.buttons.download")}</span>
                    </Button>
                  ) : (
                    <span className="inline-flex items-center gap-1 px-2 py-1 rounded-md text-[10px] text-muted-foreground bg-muted/50 shrink-0">
                      {browserVersionStatus}
                    </span>
                  )}
                </div>
              </div>

              {/* Progress bar when downloading */}
              {downloading && (
                <div className="pt-1 space-y-1.5">
                  <div className="flex items-center justify-between text-[11px] text-muted-foreground">
                    <span className="flex items-center gap-1.5">
                      <LuLoader className="size-3 animate-spin text-primary" />
                      <span>
                        {downloadProgress?.stage === "extracting"
                          ? t("createProfile.downloadStatus.extracting", {
                              defaultValue:
                                "Đang hoàn tất cài đặt trình duyệt...",
                            })
                          : downloadProgress?.stage === "verifying"
                            ? t("createProfile.downloadStatus.verifying", {
                                defaultValue:
                                  "Đang kiểm tra gói trình duyệt...",
                              })
                            : t("createProfile.downloadStatus.downloading", {
                                defaultValue:
                                  "Đang tải gói trình duyệt tự động...",
                              })}
                      </span>
                    </span>
                    <span className="font-mono text-xs font-semibold text-primary">
                      {typeof downloadProgress?.percentage === "number"
                        ? `${downloadProgress.percentage}%`
                        : "0%"}
                    </span>
                  </div>
                  <div className="h-1.5 w-full bg-muted rounded-full overflow-hidden">
                    <div
                      className="h-full bg-primary transition-all duration-300 rounded-full"
                      style={{
                        width: `${Math.max(5, downloadProgress?.percentage ?? 0)}%`,
                      }}
                    />
                  </div>
                </div>
              )}
            </div>

            {/* Section 2: Proxy & Network Routing */}
            <div className="rounded-lg border border-border/50 bg-card/40 p-3 space-y-2">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground block">
                {t("proxies.title")} &amp; Network
              </span>
              <CreateProfileProxyDialog
                proxies={storedProxies}
                selectedProxyId={selectedProxyId}
                onSelect={handleProxySelect}
                onDraftChange={handleProxyDraftChange}
              />
            </div>

            {/* Section 3: Extensions & Advanced Automation (Disclosure) */}
            <div id={advancedOptionsId} hidden={!advancedOpen}>
              <AnimatedDisclosureContent
                open={advancedOpen}
                className="space-y-3 pt-1"
              >
                <SoftFields>
                  <div className="space-y-3 rounded-lg border border-border/50 bg-card/40 p-3">
                    <span className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground block">
                      {t("createProfile.advancedOptions")}
                    </span>

                    <div className="space-y-1">
                      <Label
                        htmlFor="launch-hook-url"
                        className={FIELD_LABEL_CLASS}
                      >
                        {t("createProfile.launchHook.label")}
                      </Label>
                      <Input
                        id="launch-hook-url"
                        variant="soft"
                        value={launchHook}
                        onChange={(event) => setLaunchHook(event.target.value)}
                        placeholder={t("createProfile.launchHook.placeholder")}
                        disabled={isCreating}
                        className="h-8 text-xs"
                      />
                    </div>

                    <div
                      className={cn(
                        "grid gap-3",
                        extensionGroups.length > 0 && "sm:grid-cols-2",
                      )}
                    >
                      <div className="min-w-0 space-y-1">
                        <Label
                          htmlFor="profile-dns-blocklist"
                          className={FIELD_LABEL_CLASS}
                        >
                          {t("dnsBlocklist.title")}
                        </Label>
                        <Select
                          value={dnsBlocklist || "none"}
                          onValueChange={(value) =>
                            setDnsBlocklist(value === "none" ? "" : value)
                          }
                          disabled={isCreating}
                        >
                          <SelectTrigger
                            id="profile-dns-blocklist"
                            variant="soft"
                            className="w-full text-xs h-8"
                          >
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="none">
                              {t("dnsBlocklist.none")}
                            </SelectItem>
                            {DNS_BLOCKLIST_LEVELS.map((level) => (
                              <SelectItem key={level.value} value={level.value}>
                                {t(level.labelKey)}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>

                      {extensionGroups.length > 0 && (
                        <div className="min-w-0 space-y-1">
                          <Label
                            htmlFor="profile-extension-group"
                            className={FIELD_LABEL_CLASS}
                          >
                            {t("extensions.extensionGroup")}
                          </Label>
                          <Select
                            value={selectedExtensionGroupId ?? "none"}
                            onValueChange={(value) =>
                              setSelectedExtensionGroupId(
                                value === "none" ? undefined : value,
                              )
                            }
                            disabled={isCreating}
                          >
                            <SelectTrigger
                              id="profile-extension-group"
                              variant="soft"
                              className="w-full text-xs h-8"
                            >
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value="none">
                                {t("profileInfo.values.none")}
                              </SelectItem>
                              {extensionGroups.map((group) => (
                                <SelectItem key={group.id} value={group.id}>
                                  {group.name} ({group.extension_ids.length})
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        </div>
                      )}
                    </div>

                    <div className="space-y-3 pt-1">
                      <WayfernConfigForm
                        className="[&_[data-slot=select-trigger]]:w-full"
                        config={wayfernConfig}
                        onConfigChange={updateWayfernConfig}
                        isCreating
                        forceAdvanced
                        readOnly={isCreating}
                        profileVersion={creatableVersion}
                        profileBrowser="wayfern"
                      />
                    </div>
                  </div>
                </SoftFields>
              </AnimatedDisclosureContent>
            </div>
          </div>

          {/* Right Column: Live Fingerprint Preview Card (Compact Grid 2 Columns) */}
          <div className="hidden md:flex flex-col gap-3 rounded-xl border border-border/60 bg-gradient-to-b from-card/90 to-card/50 p-3.5 shadow-sm backdrop-blur-sm h-fit sticky top-0">
            <div className="space-y-3">
              {/* Card Header */}
              <div className="flex items-center justify-between border-b border-border/50 pb-2">
                <div className="flex items-center gap-1.5">
                  <LuShieldCheck className="size-4 text-primary" />
                  <span className="text-[11px] font-semibold tracking-wider uppercase text-foreground">
                    {t("createProfile.fingerprintSummary.title")}
                  </span>
                </div>
                <Button
                  variant="subtle"
                  size="sm"
                  className="h-6 gap-1 px-2 text-[11px] rounded-md hover:bg-primary/10 hover:text-primary transition-colors"
                  onClick={handleRandomizeFingerprint}
                  disabled={isCreating}
                  title="Randomize hardware attributes"
                >
                  <LuShuffle className="size-3" />
                  <span>{t("createProfile.fingerprintSummary.randomize")}</span>
                </Button>
              </div>

              {/* Key-Value Summary List (Compact 2-Column Mini Grid) */}
              <div className="grid grid-cols-2 gap-x-2.5 gap-y-2 text-xs">
                {/* OS & Kernel */}
                <div className="space-y-0.5">
                  <span className="text-[11px] text-muted-foreground flex items-center gap-1">
                    <SelectedOsIcon className="size-3" />
                    {t("createProfile.fingerprintSummary.osKernel")}
                  </span>
                  <span className="font-medium text-foreground text-xs truncate block">
                    {getOSDisplayName(selectedOs)} x64
                  </span>
                </div>

                {/* Browser Engine */}
                <div className="space-y-0.5">
                  <span className="text-[11px] text-muted-foreground flex items-center gap-1">
                    <LuGlobe className="size-3" />
                    {t("createProfile.fingerprintSummary.browser")}
                  </span>
                  <span className="font-medium text-foreground text-xs truncate block">
                    Browser{creatableVersion ? ` ${creatableVersion}` : ""}
                  </span>
                </div>

                {/* Display Resolution */}
                <div className="space-y-0.5">
                  <span className="text-[11px] text-muted-foreground flex items-center gap-1">
                    <LuMonitor className="size-3" />
                    {t("createProfile.fingerprintSummary.screen")}
                  </span>
                  <span className="font-mono text-[11px] text-foreground truncate block">
                    {wayfernConfig.screen_max_width &&
                    wayfernConfig.screen_max_height
                      ? `${wayfernConfig.screen_max_width} × ${wayfernConfig.screen_max_height}`
                      : t("createProfile.fingerprintSummary.screenAuto")}
                  </span>
                </div>

                {/* CPU & Memory */}
                <div className="space-y-0.5">
                  <span className="text-[11px] text-muted-foreground flex items-center gap-1">
                    <LuCpu className="size-3" />
                    {t("createProfile.fingerprintSummary.hardware")}
                  </span>
                  <span className="font-medium text-foreground text-xs truncate block">
                    {hardwareCores}C · {hardwareMemory}GB
                  </span>
                </div>

                {/* Canvas */}
                <div className="space-y-0.5">
                  <span className="text-[11px] text-muted-foreground block">
                    {t("createProfile.fingerprintSummary.canvas")}
                  </span>
                  <span className="flex items-center gap-1 text-primary text-[11px] font-medium">
                    <LuCheck className="size-3" />
                    Noise Injected
                  </span>
                </div>

                {/* WebGL */}
                <div className="space-y-0.5">
                  <span className="text-[11px] text-muted-foreground block">
                    {t("createProfile.fingerprintSummary.webgl")}
                  </span>
                  <span className="flex items-center gap-1 text-primary text-[11px] font-medium">
                    <LuCheck className="size-3" />
                    Noise Injected
                  </span>
                </div>

                {/* AudioContext */}
                <div className="space-y-0.5">
                  <span className="text-[11px] text-muted-foreground block">
                    {t("createProfile.fingerprintSummary.audio")}
                  </span>
                  <span className="flex items-center gap-1 text-primary text-[11px] font-medium">
                    <LuCheck className="size-3" />
                    Noise Injected
                  </span>
                </div>

                {/* WebRTC */}
                <div className="space-y-0.5">
                  <span className="text-[11px] text-muted-foreground block">
                    {t("createProfile.fingerprintSummary.webrtc")}
                  </span>
                  <span className="text-foreground text-[11px] truncate block">
                    Altered (Exit IP)
                  </span>
                </div>

                {/* Network / Exit IP - Full width */}
                <div className="col-span-2 pt-1 border-t border-border/30 space-y-0.5">
                  <span className="text-[11px] text-muted-foreground flex items-center gap-1">
                    <LuLayers className="size-3" />
                    {t("createProfile.fingerprintSummary.network")}
                  </span>
                  <span
                    className="font-medium text-foreground text-xs truncate block"
                    title={activeProxyName}
                  >
                    {activeProxyName}
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>

        {/* Footer */}
        <DialogFooter className="items-center justify-between pt-2 border-t border-border/50 shrink-0">
          <Button
            variant="subtle"
            size="sm"
            className="-ml-2 gap-1 rounded-lg px-2 text-xs h-8"
            aria-expanded={advancedOpen}
            aria-controls={advancedOptionsId}
            onClick={() => setAdvancedOpen((open) => !open)}
            disabled={isCreating}
          >
            <AnimatedDisclosureChevron open={advancedOpen}>
              <LuChevronRight className="size-4" />
            </AnimatedDisclosureChevron>
            {t("createProfile.advancedOptions")}
          </Button>

          <div className="ml-auto flex items-center gap-2">
            <Button
              variant="subtle"
              size="sm"
              onClick={handleClose}
              disabled={isCreating}
              className="text-xs rounded-lg h-8"
            >
              {t("common.buttons.cancel")}
            </Button>
            <LoadingButton
              onClick={handleCreate}
              isLoading={isCreating}
              disabled={isCreateDisabled}
              size="sm"
              className="h-8 rounded-lg px-4 text-xs font-medium transition-[opacity,box-shadow,transform] duration-200"
            >
              {t("common.buttons.create")}
            </LoadingButton>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

const ENTRY_SPRING = {
  type: "spring" as const,
  stiffness: 420,
  damping: 34,
  mass: 0.7,
};

const STATUS_CLASS =
  "flex items-center gap-2 rounded-lg bg-foreground/4 px-3 py-2 text-xs text-muted-foreground";

const FIELD_LABEL_CLASS = "text-xs font-medium text-muted-foreground";

function optionPill(on: boolean) {
  return cn(
    "inline-flex h-8 max-w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-full px-3 text-sm font-medium outline-none select-none transition-[background-color,color,box-shadow,transform] duration-150 ease-[var(--ease-out)] focus-visible:ring-2 focus-visible:ring-ring/50 active:scale-[0.97] disabled:pointer-events-none disabled:opacity-50 motion-reduce:active:scale-100 [&_svg]:size-3.5 [&_svg]:shrink-0",
    on
      ? "bg-primary/10 text-foreground [&_svg]:text-primary-text"
      : "bg-foreground/5 text-muted-foreground hover:bg-foreground/8 hover:text-foreground",
  );
}

function useMounted() {
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
  }, []);
  return mounted.current;
}

function PillIcon({
  on,
  animate,
  children,
}: {
  on: boolean;
  animate: boolean;
  children: React.ReactNode;
}) {
  const mounted = useMounted();
  return (
    <motion.span
      key={String(on)}
      aria-hidden="true"
      className="inline-flex"
      initial={
        animate && mounted ? { scale: 0.4, rotate: on ? -24 : 24 } : false
      }
      animate={{ scale: 1, rotate: 0 }}
      transition={{ type: "spring", stiffness: 560, damping: 22 }}
    >
      {children}
    </motion.span>
  );
}

function PillLabel({
  id,
  label,
  animate,
}: {
  id: string;
  label: string;
  animate: boolean;
}) {
  const mounted = useMounted();
  return (
    <motion.span
      key={label}
      id={id}
      className="min-w-0 truncate"
      initial={animate && mounted ? { y: 4 } : false}
      animate={{ y: 0 }}
      transition={ENTRY_SPRING}
    >
      {label}
    </motion.span>
  );
}

function OptionToggle({
  id,
  checked,
  onCheckedChange,
  disabled,
  animate,
  icon,
  label,
  hint,
}: {
  id: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled: boolean;
  animate: boolean;
  icon: React.ReactNode;
  label: string;
  hint: string;
}) {
  return (
    <Tooltip delayDuration={600}>
      <TooltipTrigger asChild>
        <CheckboxPrimitive.Root
          id={id}
          checked={checked}
          onCheckedChange={(state) => onCheckedChange(state === true)}
          disabled={disabled}
          className={optionPill(checked)}
        >
          <PillIcon on={checked} animate={animate}>
            {icon}
          </PillIcon>
          <span className="truncate">{label}</span>
        </CheckboxPrimitive.Root>
      </TooltipTrigger>
      <TooltipContent side="top" sideOffset={6} className="max-w-72">
        {hint}
      </TooltipContent>
    </Tooltip>
  );
}
