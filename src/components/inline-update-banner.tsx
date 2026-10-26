"use client";

import { useTranslation } from "react-i18next";
import { LuCheck, LuRefreshCw, LuSparkles, LuX } from "react-icons/lu";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { AppUpdateInfo } from "@/types";

export interface InlineUpdateBannerProps {
  updateInfo: AppUpdateInfo;
  onRestart: () => Promise<void>;
  onOpenDetails: () => void;
  onDismiss: () => void;
  className?: string;
}

export function InlineUpdateBanner({
  updateInfo,
  onRestart,
  onOpenDetails,
  onDismiss,
  className,
}: InlineUpdateBannerProps) {
  const { t } = useTranslation();

  return (
    <aside
      aria-label="Thông báo cập nhật ứng dụng"
      className={cn(
        "flex h-8 shrink-0 items-center justify-between border-b border-emerald-500/25 bg-emerald-950/20 px-3 text-xs text-foreground select-none transition-all duration-200 animate-in fade-in slide-in-from-top-1",
        className,
      )}
    >
      {/* Left Message */}
      <div className="flex min-w-0 items-center gap-2">
        <span className="grid size-4 shrink-0 place-items-center text-emerald-400">
          <LuSparkles className="size-3.5" />
        </span>
        <span className="truncate font-medium text-emerald-200/90 text-xs">
          BugLogin{" "}
          <span className="font-semibold text-emerald-300">
            v{updateInfo.new_version}
          </span>{" "}
          đã sẵn sàng cập nhật!
        </span>
        <button
          type="button"
          onClick={onOpenDetails}
          className="hidden sm:inline-flex text-[11px] font-medium text-emerald-400 hover:text-emerald-300 underline underline-offset-2 cursor-pointer ml-1"
        >
          {t("appUpdate.viewNotes", { defaultValue: "Xem điểm mới" })}
        </button>
      </div>

      {/* Right Actions */}
      <div className="flex shrink-0 items-center gap-1.5 ml-2">
        <Button
          size="sm"
          variant="ghost"
          onClick={onDismiss}
          className="h-6 px-2 text-[11px] text-muted-foreground hover:text-foreground hover:bg-emerald-500/10 cursor-pointer"
        >
          {t("common.actions.later", { defaultValue: "Để sau" })}
        </Button>

        <Button
          size="sm"
          onClick={() => void onRestart()}
          className="h-6 px-2.5 text-[11px] font-semibold bg-emerald-600 hover:bg-emerald-500 text-white shadow-2xs gap-1 cursor-pointer"
        >
          <LuRefreshCw className="size-3" />
          <span>
            {t("appUpdate.restartNow", { defaultValue: "Khởi động lại ngay" })}
          </span>
        </Button>

        <button
          type="button"
          onClick={onDismiss}
          aria-label={t("common.actions.close", { defaultValue: "Đóng" })}
          className="grid size-5 place-items-center rounded text-muted-foreground/70 hover:text-foreground hover:bg-muted/40 transition-colors cursor-pointer ml-0.5"
        >
          <LuX className="size-3" />
        </button>
      </div>
    </aside>
  );
}

export default InlineUpdateBanner;
