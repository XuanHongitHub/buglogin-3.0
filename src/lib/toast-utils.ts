import { invoke } from "@tauri-apps/api/core";
import React from "react";
import { type ExternalToast, toast as sonnerToast } from "sonner";
import { UnifiedToast } from "@/components/custom-toast";
import i18n from "@/i18n";
import { formatError } from "./error-utils";

interface BaseToastProps {
  id?: string;
  title: string;
  description?: string;
  duration?: number;
  action?: ExternalToast["action"];
  onCancel?: () => void;
  // When false, the toast cannot be dismissed by the user (no swipe; combine
  // with duration: Infinity and no onCancel to make it fully non-closable).
  dismissible?: boolean;
}

interface LoadingToastProps extends BaseToastProps {
  type: "loading";
}

interface SuccessToastProps extends BaseToastProps {
  type: "success";
}

interface ErrorToastProps extends BaseToastProps {
  type: "error";
}

interface InfoToastProps extends BaseToastProps {
  type: "info";
}

interface DownloadToastProps extends BaseToastProps {
  type: "download";
  stage?: "downloading" | "extracting" | "verifying" | "completed";
  progress?: {
    percentage: number;
    speed?: string;
    eta?: string;
  };
}

interface VersionUpdateToastProps extends BaseToastProps {
  type: "version-update";
  progress?: {
    current: number;
    total: number;
    found: number;
    current_browser?: string;
  };
}

interface SyncProgressToastProps extends BaseToastProps {
  type: "sync-progress";
  progress?: {
    completed_files: number;
    total_files: number;
    completed_bytes: number;
    total_bytes: number;
    speed_bytes_per_sec: number;
    eta_seconds: number;
    failed_count: number;
    phase: string;
  };
}

type ToastProps =
  | SuccessToastProps
  | ErrorToastProps
  | InfoToastProps
  | DownloadToastProps
  | LoadingToastProps
  | VersionUpdateToastProps
  | SyncProgressToastProps;

function cleanToastText(text: unknown): string {
  if (text == null) return "";
  const formatted = formatError(text);
  if (formatted.includes("[object Object]")) {
    return formatted.replace(/\[object Object\]/g, "Đã xảy ra lỗi");
  }
  return formatted;
}

export function showToast(props: ToastProps & { id?: string }) {
  const toastId = props.id ?? `toast-${props.type}-${Date.now()}`;

  const sanitizedProps = {
    ...props,
    title: cleanToastText(props.title),
    description: props.description
      ? cleanToastText(props.description)
      : undefined,
  };

  let duration: number;
  if (sanitizedProps.duration !== undefined) {
    duration = sanitizedProps.duration;
  } else {
    switch (sanitizedProps.type) {
      case "loading":
        duration = 10000;
        break;
      case "download":
        if ("stage" in sanitizedProps && sanitizedProps.stage === "completed") {
          duration = 3000;
        } else {
          duration = Number.POSITIVE_INFINITY;
        }
        break;
      case "success":
        duration = 3000;
        break;
      case "error":
        duration = 10000;
        break;
      case "version-update":
        duration = 15000;
        break;
      case "sync-progress":
        duration = Number.POSITIVE_INFINITY;
        break;
      default:
        duration = 5000;
    }
  }

  sonnerToast.custom(() => React.createElement(UnifiedToast, sanitizedProps), {
    id: toastId,
    duration,
    dismissible: sanitizedProps.dismissible,
    style: {
      background: "transparent",
      border: "none",
      boxShadow: "none",
      padding: 0,
      zIndex: 10001,
      pointerEvents: "auto",
    },
  });

  return toastId;
}

export function showDownloadToast(
  browserName: string,
  version: string,
  stage: "downloading" | "extracting" | "verifying" | "completed",
  progress?: { percentage: number; speed?: string; eta?: string },
  options?: { suppressCompletionToast?: boolean; onCancel?: () => void },
) {
  const tParams = { browser: browserName, version };
  const title =
    stage === "completed"
      ? i18n.t("toasts.success.downloadComplete", tParams)
      : stage === "downloading"
        ? i18n.t("toasts.loading.downloading", tParams)
        : stage === "extracting"
          ? i18n.t("toasts.loading.extracting", tParams)
          : i18n.t("toasts.loading.verifying", tParams);

  if (stage === "completed" && options?.suppressCompletionToast) {
    dismissToast(`download-${browserName.toLowerCase()}-${version}`);
    return;
  }

  const showCancel = stage === "downloading";

  return showToast({
    type: "download",
    title,
    stage,
    progress,
    id: `download-${browserName.toLowerCase()}-${version}`,
    onCancel: showCancel ? options?.onCancel : undefined,
  });
}

export function showSuccessToast(
  title: unknown,
  options?: {
    id?: string;
    description?: unknown;
    duration?: number;
  },
) {
  return showToast({
    type: "success",
    title: cleanToastText(title),
    ...options,
    description: options?.description
      ? cleanToastText(options.description)
      : undefined,
  });
}

export function showErrorToast(
  title: unknown,
  options?: {
    id?: string;
    description?: unknown;
    duration?: number;
  },
) {
  return showToast({
    type: "error",
    title: cleanToastText(title),
    ...options,
    description: options?.description
      ? cleanToastText(options.description)
      : undefined,
  });
}

export function showAutoUpdateToast(
  browserName: string,
  version: string,
  options?: {
    id?: string;
    description?: string;
    duration?: number;
  },
) {
  return showToast({
    type: "loading",
    title: i18n.t("versionUpdater.toast.updateStarted", {
      browser: browserName,
    }),
    description:
      options?.description ??
      i18n.t("versionUpdater.toast.autoDownloadStarted", {
        browser: browserName,
        version,
      }),
    id: options?.id ?? `auto-update-${browserName.toLowerCase()}-${version}`,
    duration: options?.duration ?? 4000,
  });
}

export function dismissToast(id: string) {
  sonnerToast.dismiss(id);
}

export function showSyncProgressToast(
  profileName: string,
  progress: {
    completed_files: number;
    total_files: number;
    completed_bytes: number;
    total_bytes: number;
    speed_bytes_per_sec: number;
    eta_seconds: number;
    failed_count: number;
    phase: string;
  },
  options?: { id?: string; profileId?: string },
) {
  return showToast({
    type: "sync-progress",
    title: i18n.t("toasts.loading.syncingProfile", { name: profileName }),
    progress,
    id: options?.id,
    duration: Number.POSITIVE_INFINITY,
    onCancel: () => {
      if (options?.profileId) {
        void invoke("cancel_profile_sync", {
          profileId: options.profileId,
        }).catch((err: unknown) => {
          console.error("Failed to cancel sync:", err);
        });
      }
      if (options?.id) {
        dismissToast(options.id);
      }
    },
  });
}
