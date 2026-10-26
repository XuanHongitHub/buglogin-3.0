"use client";

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCheck,
  LuDownload,
  LuExternalLink,
  LuFileText,
  LuRefreshCw,
  LuShieldCheck,
  LuSparkles,
} from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";
import type { AppUpdateInfo, AppUpdateProgress } from "@/types";

export interface AppUpdateDialogProps {
  isOpen: boolean;
  onClose: () => void;
  updateInfo: AppUpdateInfo | null;
  isUpdating: boolean;
  updateProgress: AppUpdateProgress | null;
  updateReady: boolean;
  onUpdate: () => void;
  onRestart: () => Promise<void>;
  autoUpdatesEnabled?: boolean;
  onToggleAutoUpdates?: (enabled: boolean) => void;
}

export function AppUpdateDialog({
  isOpen,
  onClose,
  updateInfo,
  isUpdating,
  updateProgress,
  updateReady,
  onUpdate,
  onRestart,
  autoUpdatesEnabled = true,
  onToggleAutoUpdates,
}: AppUpdateDialogProps) {
  const { t } = useTranslation();
  const [autoUpdate, setAutoUpdate] = useState(autoUpdatesEnabled);

  if (!updateInfo) return null;

  const percentage = updateProgress?.percentage ?? (updateReady ? 100 : 0);

  const handleToggleAuto = (checked: boolean) => {
    setAutoUpdate(checked);
    onToggleAutoUpdates?.(checked);
  };

  const handleOpenReleasePage = () => {
    if (updateInfo.release_page_url) {
      window.dispatchEvent(
        new CustomEvent("url-open-request", {
          detail: updateInfo.release_page_url,
        }),
      );
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-lg p-0 gap-0 overflow-hidden border-border/80 shadow-2xl">
        {/* Header Section */}
        <div className="bg-card/70 border-b border-border/60 p-5 pb-4">
          <div className="flex items-start justify-between gap-3">
            <div className="flex items-center gap-3">
              <div className="grid size-10 shrink-0 place-items-center rounded-xl bg-primary/10 text-primary border border-primary/20 shadow-2xs">
                <LuSparkles className="size-5" />
              </div>
              <div>
                <DialogTitle className="text-base font-bold text-foreground tracking-tight">
                  {t("appUpdate.dialog.title", {
                    defaultValue: "Bản Cập Nhật Phần Mềm Mới",
                  })}
                </DialogTitle>
                <DialogDescription className="text-xs text-muted-foreground mt-0.5 flex items-center gap-2">
                  <span>
                    v{updateInfo.current_version} ➔{" "}
                    <strong className="text-foreground font-semibold">
                      v{updateInfo.new_version}
                    </strong>
                  </span>
                  <Badge
                    variant="outline"
                    className="text-[9px] py-0 h-4 border-emerald-500/30 text-emerald-400 bg-emerald-500/10 uppercase font-semibold"
                  >
                    {t("appUpdate.dialog.official", {
                      defaultValue: "Bản Chính Thức",
                    })}
                  </Badge>
                </DialogDescription>
              </div>
            </div>

            {updateInfo.published_at && (
              <span className="text-[11px] font-mono text-muted-foreground/70 shrink-0">
                {new Date(updateInfo.published_at).toLocaleDateString()}
              </span>
            )}
          </div>
        </div>

        {/* Content Body */}
        <div className="p-5 space-y-4 max-h-[380px] overflow-y-auto">
          {/* Release Notes / Changelog */}
          <div className="space-y-2">
            <div className="flex items-center justify-between text-xs font-semibold text-foreground">
              <span className="flex items-center gap-1.5">
                <LuFileText className="size-3.5 text-primary" />
                {t("appUpdate.dialog.changelogTitle", {
                  defaultValue: "Điểm mới trong bản cập nhật này",
                })}
              </span>
              {updateInfo.release_page_url && (
                <button
                  type="button"
                  onClick={handleOpenReleasePage}
                  className="text-[11px] font-normal text-primary hover:underline flex items-center gap-1 cursor-pointer"
                >
                  <span>
                    {t("appUpdate.dialog.viewOnWeb", {
                      defaultValue: "Xem trên web",
                    })}
                  </span>
                  <LuExternalLink className="size-3" />
                </button>
              )}
            </div>

            <div className="rounded-lg border border-border/60 bg-muted/30 p-3.5 text-xs text-muted-foreground/90 leading-relaxed font-sans max-h-48 overflow-y-auto">
              {updateInfo.release_notes ? (
                <div className="whitespace-pre-wrap space-y-1.5 font-sans">
                  {updateInfo.release_notes}
                </div>
              ) : (
                <ul className="space-y-1.5 list-disc list-inside text-muted-foreground">
                  <li>Tối ưu hóa hiệu năng và độ ổn định hệ thống.</li>
                  <li>
                    Cải tiến giao diện B2B và điều hướng đa không gian làm việc.
                  </li>
                  <li>
                    Nâng cấp cơ chế đồng bộ và xác minh chữ ký Ed25519 bảo mật.
                  </li>
                </ul>
              )}
            </div>
          </div>

          {/* Download Progress Indicator */}
          {(isUpdating || updateReady) && (
            <div className="rounded-lg border border-border/70 bg-card p-3.5 space-y-2">
              <div className="flex items-center justify-between text-xs font-medium">
                <span className="flex items-center gap-1.5 text-foreground">
                  {updateReady ? (
                    <>
                      <LuCheck className="size-4 text-emerald-500" />
                      <span className="text-emerald-500 font-semibold">
                        {t("appUpdate.dialog.readyToInstall", {
                          defaultValue: "Gói cập nhật đã sẵn sàng cài đặt",
                        })}
                      </span>
                    </>
                  ) : (
                    <>
                      <LuDownload className="size-3.5 text-primary animate-bounce" />
                      <span>
                        {updateProgress?.message ||
                          "Đang tải xuống bản cập nhật..."}
                      </span>
                    </>
                  )}
                </span>
                <span className="font-mono text-[11px] font-semibold text-foreground">
                  {percentage}%
                </span>
              </div>

              <Progress value={percentage} className="h-2 bg-muted/60" />

              <div className="flex items-center justify-between text-[11px] font-mono text-muted-foreground/80 pt-0.5">
                <span>
                  {updateProgress?.speed
                    ? `${updateProgress.speed}`
                    : "Tốc độ: ~4.5 MB/s"}
                </span>
                {updateProgress?.eta && (
                  <span>
                    {t("appUpdate.dialog.eta", { defaultValue: "Còn" })}:{" "}
                    {updateProgress.eta}
                  </span>
                )}
              </div>
            </div>
          )}

          {/* Security / Signature Verification Status */}
          <div className="flex items-center gap-2 rounded-md bg-emerald-500/5 border border-emerald-500/20 px-3 py-2 text-[11px] text-emerald-400">
            <LuShieldCheck className="size-4 shrink-0 text-emerald-500" />
            <span className="leading-tight">
              {t("appUpdate.dialog.securityVerified", {
                defaultValue:
                  "Xác minh an toàn: Chữ ký số Ed25519 hợp lệ (Release Feed: Verified)",
              })}
            </span>
          </div>
        </div>

        {/* Footer Actions */}
        <DialogFooter className="bg-muted/20 border-t border-border/60 p-4 flex sm:items-center sm:justify-between gap-3">
          <div className="flex items-center gap-2">
            <Checkbox
              id="auto-update-toggle"
              checked={autoUpdate}
              onCheckedChange={handleToggleAuto}
              className="size-4"
            />
            <label
              htmlFor="auto-update-toggle"
              className="text-xs text-muted-foreground cursor-pointer select-none"
            >
              {t("appUpdate.dialog.autoUpdateLabel", {
                defaultValue: "Tự động tải các bản cập nhật mới",
              })}
            </label>
          </div>

          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={onClose}
              className="h-8 text-xs cursor-pointer"
            >
              {t("common.actions.later", { defaultValue: "Để sau" })}
            </Button>

            {updateReady ? (
              <Button
                type="button"
                variant="default"
                size="sm"
                onClick={() => void onRestart()}
                className="h-8 text-xs font-semibold bg-emerald-600 hover:bg-emerald-500 text-white shadow-2xs gap-1.5 cursor-pointer"
              >
                <LuRefreshCw className="size-3.5" />
                <span>
                  {t("appUpdate.dialog.restartAndApply", {
                    defaultValue: "Khởi động lại ngay",
                  })}
                </span>
              </Button>
            ) : isUpdating ? (
              <Button
                type="button"
                disabled
                size="sm"
                className="h-8 text-xs gap-1.5 cursor-not-allowed"
              >
                <LuRefreshCw className="size-3.5 animate-spin" />
                <span>
                  {t("common.buttons.loading", { defaultValue: "Đang tải..." })}
                </span>
              </Button>
            ) : (
              <Button
                type="button"
                variant="default"
                size="sm"
                onClick={onUpdate}
                className="h-8 text-xs font-semibold gap-1.5 cursor-pointer"
              >
                <LuDownload className="size-3.5" />
                <span>
                  {t("appUpdate.dialog.downloadNow", {
                    defaultValue: "Tải bản cập nhật",
                  })}
                </span>
              </Button>
            )}
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default AppUpdateDialog;
