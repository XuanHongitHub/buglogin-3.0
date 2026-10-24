"use client";

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuBell, LuCheck, LuInfo } from "react-icons/lu";
import { LoadingButton } from "@/components/loading-button";
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
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import { RippleButton } from "./ui/ripple";

interface PendingInvitesDialogProps {
  isOpen: boolean;
  onClose: () => void;
  onInviteAccepted?: (workspaceId: string) => void;
  initialCode?: string;
}

const desktopApi = createTauriDesktopApi();

export function PendingInvitesDialog({
  isOpen,
  onClose,
  onInviteAccepted,
  initialCode,
}: PendingInvitesDialogProps) {
  const { t } = useTranslation();
  const [inviteCode, setInviteCode] = useState("");
  const [isRedeeming, setIsRedeeming] = useState(false);

  useEffect(() => {
    setInviteCode(initialCode ?? "");
  }, [initialCode]);

  const handleRedeemCode = async (e: React.FormEvent) => {
    e.preventDefault();
    const code = inviteCode.trim();
    if (!code) return;

    setIsRedeeming(true);
    try {
      let invitationId = "";
      let invitationCode = "";

      if (code.startsWith("buglogin://invite")) {
        const url = new URL(code);
        invitationId = url.searchParams.get("invitationId") || "";
        invitationCode = url.searchParams.get("code") || "";
      } else {
        const separator = code.indexOf(".");
        if (separator > 0) {
          invitationId = code.slice(0, separator);
          invitationCode = code.slice(separator + 1);
        }
      }
      if (!invitationId || !invitationCode) {
        throw new Error(
          "Enter the invitation ID and one-time code, or paste the invite link.",
        );
      }

      const pendingMember = await desktopApi.redeemMemberInvitation({
        invitationId,
        code: invitationCode,
      });
      await desktopApi.activateMember({
        workspaceId: pendingMember.workspaceId,
        memberId: pendingMember.id,
      });

      showSuccessToast(
        t("invites.redeemSuccess", {
          defaultValue:
            "Kích hoạt lời mời thành công! Chào mừng bạn tham gia workspace.",
        }),
      );
      setInviteCode("");
      if (onInviteAccepted) onInviteAccepted(pendingMember.workspaceId);
      onClose();
    } catch (err) {
      console.error("Failed to redeem invite code:", err);
      showErrorToast(
        t("invites.redeemFailed", {
          defaultValue:
            "Mã mời không hợp lệ hoặc đã hết hạn. Vui lòng kiểm tra lại.",
        }),
      );
    } finally {
      setIsRedeeming(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="max-w-lg p-6">
        <DialogHeader className="border-b border-border/50 pb-3">
          <div className="flex items-center gap-3">
            <div className="grid size-10 place-items-center rounded-xl bg-primary/10 text-primary shadow-xs">
              <LuBell className="size-5" />
            </div>
            <div>
              <DialogTitle className="text-base font-semibold">
                {t("invites.title", {
                  defaultValue: "Hộp Thư Lời Mời & Kích Hoạt Workspace",
                })}
              </DialogTitle>
              <DialogDescription className="text-xs text-muted-foreground mt-0.5">
                {t("invites.subtitle", {
                  defaultValue:
                    "Nhập mã mời trực tiếp hoặc nhấp liên kết để tham gia không gian làm việc",
                })}
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <div className="space-y-4 py-2 text-xs">
          {/* Direct Code Input Box */}
          <form
            onSubmit={(e) => void handleRedeemCode(e)}
            className="space-y-3"
          >
            <div className="space-y-1.5">
              <Label className="text-xs font-medium text-foreground">
                Nhập Mã Mời Trực Tiếp (Invite Code hoặc Deep Link):
              </Label>
              <div className="flex items-center gap-2">
                <Input
                  placeholder="Dán mã invitationId.code hoặc link mời"
                  value={inviteCode}
                  onChange={(e) => setInviteCode(e.target.value)}
                  required
                  className="h-9 font-mono text-xs"
                />
                <LoadingButton
                  type="submit"
                  size="sm"
                  isLoading={isRedeeming}
                  disabled={!inviteCode.trim()}
                  className="h-9 gap-1.5 px-3.5 text-xs font-semibold cursor-pointer shrink-0"
                >
                  <LuCheck className="size-3.5" />
                  <span>Tham Gia</span>
                </LoadingButton>
              </div>
            </div>
          </form>

          <div className="space-y-2 pt-1">
            <span className="font-semibold text-foreground text-xs">
              Hộp thư lời mời
            </span>
            <p className="rounded-lg border border-border/70 bg-muted/20 p-3 text-xs text-muted-foreground">
              Chưa có API tải danh sách lời mời đang chờ. Dán mã hoặc liên kết
              mời ở phía trên để xác minh và tham gia; không có lời mời mẫu nào
              được hiển thị.
            </p>
          </div>

          {/* Note Callout */}
          <div className="rounded-lg border border-primary/20 bg-primary/5 p-3 flex items-start gap-2.5 text-xs text-foreground/85">
            <LuInfo className="size-4 text-primary shrink-0 mt-0.5" />
            <div className="space-y-0.5">
              <span className="font-semibold text-primary">
                Quyền lợi sau khi tham gia:
              </span>
              <p className="text-[11px] text-muted-foreground leading-relaxed">
                Sau khi kích hoạt mã mời, Không Gian Làm Việc sẽ tự động xuất
                hiện trong menu chuyển đổi Workspace ở thanh điều hướng bên
                trái. Bạn sẽ có quyền truy cập ngay vào các hồ sơ trình duyệt
                được phân công.
              </p>
            </div>
          </div>
        </div>

        <DialogFooter className="border-t border-border/50 pt-3">
          <RippleButton
            variant="outline"
            size="sm"
            onClick={onClose}
            className="h-8 text-xs font-medium cursor-pointer"
          >
            {t("common.buttons.close", { defaultValue: "Đóng" })}
          </RippleButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
