"use client";

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuBuilding2,
  LuCheck,
  LuCopy,
  LuMail,
  LuSend,
  LuSparkles,
} from "react-icons/lu";
import { LoadingButton } from "@/components/loading-button";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type { MemberRole } from "../../frontend/src/generated/command-registry";
import { RippleButton } from "./ui/ripple";

interface CreateInviteModalProps {
  isOpen: boolean;
  onClose: () => void;
  workspaceId?: string;
  workspaceName?: string;
  onInviteCreated?: () => void;
}

const desktopApi = createTauriDesktopApi();

export function CreateInviteModal({
  isOpen,
  onClose,
  workspaceId: propWorkspaceId,
  workspaceName: propWorkspaceName,
  onInviteCreated,
}: CreateInviteModalProps) {
  const { t } = useTranslation();

  const [workspaces, setWorkspaces] = useState<
    Array<{ id: string; name: string; selected?: boolean }>
  >([]);
  const [selectedWsId, setSelectedWsId] = useState(propWorkspaceId || "");
  const [recipientEmail, setRecipientEmail] = useState("");
  const [selectedRole, setSelectedRole] = useState<MemberRole>("operator");
  const [durationDays, setDurationDays] = useState("7");

  const [isGenerating, setIsGenerating] = useState(false);
  const [generatedResult, setGeneratedResult] = useState<{
    code: string;
    deepLink: string;
    expiresAt: string;
    wsName: string;
  } | null>(null);

  const [copiedCode, setCopiedCode] = useState(false);
  const [copiedLink, setCopiedLink] = useState(false);
  const [copiedFullMessage, setCopiedFullMessage] = useState(false);

  // Load only workspaces visible through the authenticated API.
  useEffect(() => {
    let disposed = false;
    const loadWorkspaces = async () => {
      try {
        const organizations = await desktopApi.listOrganizations();
        const pages = await Promise.all(
          organizations.items.map((organization) =>
            desktopApi.listWorkspaces(organization.id),
          ),
        );
        const workspacesForAccount = pages.flatMap((page) => page.items);
        if (disposed) return;
        setWorkspaces(
          workspacesForAccount.map((workspace) => ({
            id: workspace.id,
            name: workspace.name,
            selected: workspace.id === propWorkspaceId,
          })),
        );
        setSelectedWsId((current) => {
          if (
            propWorkspaceId &&
            workspacesForAccount.some(
              (workspace) => workspace.id === propWorkspaceId,
            )
          ) {
            return propWorkspaceId;
          }
          if (
            current &&
            workspacesForAccount.some((workspace) => workspace.id === current)
          ) {
            return current;
          }
          return workspacesForAccount[0]?.id ?? "";
        });
      } catch (error) {
        if (!disposed) {
          console.warn(
            "Failed to load invitation workspaces through the API:",
            error,
          );
          showErrorToast(String(error));
        }
      }
    };
    void loadWorkspaces();

    return () => {
      disposed = true;
    };
  }, [propWorkspaceId]);

  const handleCreateInvite = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!recipientEmail.trim() || !selectedWsId) return;

    setIsGenerating(true);
    try {
      const activeWs = workspaces.find((w) => w.id === selectedWsId);
      const wsName =
        propWorkspaceName || activeWs?.name || "Workspace Hiện Tại";

      const days = Number(durationDays) || 7;
      const expireDate = new Date();
      expireDate.setDate(expireDate.getDate() + days);
      const invitation = await desktopApi.createMemberInvitation({
        workspaceId: selectedWsId,
        email: recipientEmail.trim(),
        role: selectedRole,
        expiresAt: Math.floor(expireDate.getTime() / 1000),
      });
      if (!invitation.code) {
        throw new Error(
          "The invitation was already created, but its one-time code is no longer available. Create a new invitation.",
        );
      }
      const code = `${invitation.invitationId}.${invitation.code}`;
      const deepLink = `buglogin://invite?invitationId=${encodeURIComponent(invitation.invitationId)}&code=${encodeURIComponent(invitation.code)}`;

      setGeneratedResult({
        code,
        deepLink,
        expiresAt: new Date(invitation.expiresAt * 1000).toLocaleDateString(
          "vi-VN",
          {
            year: "numeric",
            month: "2-digit",
            day: "2-digit",
            hour: "2-digit",
            minute: "2-digit",
          },
        ),
        wsName,
      });

      showSuccessToast(
        t("invites.createdSuccess", {
          defaultValue: "Đã tạo mã mời và liên kết tham gia thành công!",
        }),
      );

      if (onInviteCreated) onInviteCreated();
    } catch (err) {
      console.error("Failed to generate invite:", err);
      showErrorToast(String(err));
    } finally {
      setIsGenerating(false);
    }
  };

  const handleCopyCode = async () => {
    if (!generatedResult) return;
    try {
      await navigator.clipboard.writeText(generatedResult.code);
      setCopiedCode(true);
      showSuccessToast(
        t("common.copied", { defaultValue: "Đã sao chép mã mời" }),
      );
      setTimeout(() => setCopiedCode(false), 2000);
    } catch (err) {
      showErrorToast(String(err));
    }
  };

  const handleCopyLink = async () => {
    if (!generatedResult) return;
    try {
      await navigator.clipboard.writeText(generatedResult.deepLink);
      setCopiedLink(true);
      showSuccessToast(
        t("common.copied", { defaultValue: "Đã sao chép liên kết" }),
      );
      setTimeout(() => setCopiedLink(false), 2000);
    } catch (err) {
      showErrorToast(String(err));
    }
  };

  const handleCopyFullMessage = async () => {
    if (!generatedResult) return;
    const msg = [
      `Bạn được mời tham gia Không Gian Làm Việc "${generatedResult.wsName}" trên nền tảng BugLogin!`,
      `• Vai trò: ${selectedRole.toUpperCase()}`,
      `• Mã mời trực tiếp: ${generatedResult.code}`,
      `• Liên kết tham gia (Deep Link): ${generatedResult.deepLink}`,
      `• Hạn chót: ${generatedResult.expiresAt}`,
      `\nHướng dẫn: Mở ứng dụng BugLogin Desktop, dán mã mời hoặc click vào liên kết để bắt đầu làm việc ngay.`,
    ].join("\n");

    try {
      await navigator.clipboard.writeText(msg);
      setCopiedFullMessage(true);
      showSuccessToast(
        t("invites.fullCopied", {
          defaultValue:
            "Đã sao chép toàn bộ tin nhắn để gửi qua Zalo / Telegram!",
        }),
      );
      setTimeout(() => setCopiedFullMessage(false), 2000);
    } catch (err) {
      showErrorToast(String(err));
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="max-w-xl p-6">
        <DialogHeader className="border-b border-border/50 pb-3">
          <div className="flex items-center gap-3">
            <div className="grid size-10 place-items-center rounded-xl bg-primary/10 text-primary shadow-xs">
              <LuSend className="size-5" />
            </div>
            <div>
              <DialogTitle className="text-base font-semibold">
                {t("invites.createTitle", {
                  defaultValue: "Mời Thành Viên Vào Không Gian Làm Việc",
                })}
              </DialogTitle>
              <DialogDescription className="text-xs text-muted-foreground mt-0.5">
                {t("invites.createDesc", {
                  defaultValue:
                    "Tạo liên kết và mã mời thành viên tham gia workspace",
                })}
                <span className="mt-1 block">
                  Người nhận cần account BugLogin đang hoạt động với email đã
                  xác minh. Lời mời không tạo account hay mật khẩu.
                </span>
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        {!generatedResult ? (
          <form
            onSubmit={(e) => void handleCreateInvite(e)}
            className="space-y-4 py-2 text-xs"
          >
            {/* Workspace Selection */}
            <div className="space-y-1.5">
              <Label className="text-xs font-medium text-foreground">
                {t("invites.workspaceLabel", {
                  defaultValue: "Không gian làm việc",
                })}
              </Label>
              <Select value={selectedWsId} onValueChange={setSelectedWsId}>
                <SelectTrigger className="h-9 text-xs">
                  <SelectValue
                    placeholder={t("invites.selectWorkspacePlaceholder", {
                      defaultValue: "Chọn không gian...",
                    })}
                  />
                </SelectTrigger>
                <SelectContent>
                  {workspaces.map((ws) => (
                    <SelectItem key={ws.id} value={ws.id} className="text-xs">
                      <span className="inline-flex items-center gap-1.5">
                        <LuBuilding2 className="size-3.5 text-muted-foreground" />
                        {ws.name}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {/* Recipient Email */}
            <div className="space-y-1.5">
              <Label className="text-xs font-medium text-foreground">
                {t("invites.recipientEmailLabel", {
                  defaultValue: "Email người nhận *",
                })}
              </Label>
              <div className="relative flex items-center">
                <LuMail className="absolute left-3 size-4 text-muted-foreground" />
                <Input
                  type="email"
                  placeholder="member@company.com"
                  value={recipientEmail}
                  onChange={(e) => setRecipientEmail(e.target.value)}
                  required
                  className="h-9 pl-9 text-xs"
                />
              </div>
            </div>

            {/* Role Selection */}
            <div className="space-y-1.5">
              <Label className="text-xs font-medium text-foreground">
                {t("invites.roleLabel", { defaultValue: "Vai trò" })}
              </Label>
              <div className="grid grid-cols-2 gap-2">
                {[
                  {
                    id: "admin" as MemberRole,
                    label: t("roles.admin", { defaultValue: "Quản trị viên" }),
                    desc: t("roles.adminDesc", {
                      defaultValue: "Toàn quyền quản trị",
                    }),
                  },
                  {
                    id: "lead" as MemberRole,
                    label: "Lead",
                    desc: "Quản lý thành viên, đội nhóm và tài nguyên cấp thấp hơn",
                  },
                  {
                    id: "member" as MemberRole,
                    label: t("roles.member", { defaultValue: "Thành viên" }),
                    desc: t("roles.memberDesc", {
                      defaultValue: "Quyền theo phân công và grant được cấp",
                    }),
                  },
                  {
                    id: "operator" as MemberRole,
                    label: t("roles.operator", { defaultValue: "Thành viên" }),
                    desc: t("roles.operatorDesc", {
                      defaultValue: "Khởi chạy hồ sơ được gán",
                    }),
                  },
                ].map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    onClick={() => setSelectedRole(item.id)}
                    className={cn(
                      "flex flex-col items-start p-2.5 rounded-lg border text-left transition-all cursor-pointer",
                      selectedRole === item.id
                        ? "border-primary bg-primary/10 text-primary"
                        : "border-border/70 hover:bg-muted/30 text-foreground",
                    )}
                  >
                    <span className="font-semibold text-xs">{item.label}</span>
                    <span className="text-[10px] text-muted-foreground mt-0.5">
                      {item.desc}
                    </span>
                  </button>
                ))}
              </div>
            </div>

            {/* Expiration */}
            <div className="space-y-1.5">
              <Label className="text-xs font-medium text-foreground">
                Thời hạn mã mời:
              </Label>
              <Select value={durationDays} onValueChange={setDurationDays}>
                <SelectTrigger className="h-9 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="7" className="text-xs">
                    7 ngày (Tiêu chuẩn)
                  </SelectItem>
                  <SelectItem value="14" className="text-xs">
                    14 ngày (2 tuần)
                  </SelectItem>
                  <SelectItem value="30" className="text-xs">
                    30 ngày (1 tháng)
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <DialogFooter className="border-t border-border/50 pt-3 flex items-center justify-between sm:justify-between">
              <Button
                variant="ghost"
                size="sm"
                type="button"
                onClick={onClose}
                className="h-8 text-xs cursor-pointer"
              >
                Hủy
              </Button>
              <LoadingButton
                type="submit"
                size="sm"
                isLoading={isGenerating}
                disabled={!recipientEmail.trim() || !selectedWsId}
                className="h-8 gap-1.5 px-4 text-xs font-medium cursor-pointer"
              >
                <LuSparkles className="size-3.5" />
                <span>Sinh Mã Mời & Link</span>
              </LoadingButton>
            </DialogFooter>
          </form>
        ) : (
          /* GENERATED RESULT BLOCK (Section 14.1) */
          <div className="space-y-4 py-2 text-xs">
            <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 p-3 space-y-1">
              <div className="flex items-center gap-2 text-emerald-600 font-semibold text-xs">
                <LuCheck className="size-4" />
                <span>Mã Mời Đã Sẵn Sàng Gửi Đi</span>
              </div>
              <p className="text-[11px] text-muted-foreground">
                Không cần cấu hình SMTP máy chủ gửi mail. Hãy sao chép mã hoặc
                liên kết dưới đây để gửi trực tiếp cho nhân sự qua Telegram,
                Zalo hoặc Slack.
              </p>
            </div>

            {/* Invite Code Field */}
            <div className="space-y-1.5">
              <Label className="text-xs font-medium text-foreground">
                Mã Mời (Invite Code):
              </Label>
              <div className="flex items-center gap-2">
                <Input
                  readOnly
                  value={generatedResult.code}
                  className="h-9 font-mono font-bold tracking-wider text-xs bg-muted/20"
                />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void handleCopyCode()}
                  className="h-9 px-3 gap-1 text-xs shrink-0 cursor-pointer"
                >
                  {copiedCode ? (
                    <LuCheck className="size-3.5 text-emerald-500" />
                  ) : (
                    <LuCopy className="size-3.5" />
                  )}
                  <span>{copiedCode ? "Đã chép" : "Copy"}</span>
                </Button>
              </div>
            </div>

            {/* Deep Link Field */}
            <div className="space-y-1.5">
              <Label className="text-xs font-medium text-foreground">
                Liên Kết Tham Gia Trực Tiếp (Deep Link):
              </Label>
              <div className="flex items-center gap-2">
                <Input
                  readOnly
                  value={generatedResult.deepLink}
                  className="h-9 font-mono text-[11px] bg-muted/20 text-muted-foreground"
                />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void handleCopyLink()}
                  className="h-9 px-3 gap-1 text-xs shrink-0 cursor-pointer"
                >
                  {copiedLink ? (
                    <LuCheck className="size-3.5 text-emerald-500" />
                  ) : (
                    <LuCopy className="size-3.5" />
                  )}
                  <span>{copiedLink ? "Đã chép" : "Copy"}</span>
                </Button>
              </div>
            </div>

            {/* Expiry Note */}
            <div className="flex items-center justify-between text-[11px] text-muted-foreground px-1">
              <span>
                Hạn sử dụng: <strong>{generatedResult.expiresAt}</strong>
              </span>
              <span>
                Người nhận: <strong>{recipientEmail}</strong>
              </span>
            </div>

            <DialogFooter className="border-t border-border/50 pt-3 flex items-center justify-between sm:justify-between">
              <RippleButton
                variant="outline"
                size="sm"
                onClick={onClose}
                className="h-8 text-xs font-medium cursor-pointer"
              >
                Xong / Đóng
              </RippleButton>

              <Button
                size="sm"
                onClick={() => void handleCopyFullMessage()}
                className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
              >
                {copiedFullMessage ? (
                  <LuCheck className="size-3.5 text-emerald-500" />
                ) : (
                  <LuCopy className="size-3.5" />
                )}
                <span>Sao Chép Toàn Bộ Tin Nhắn</span>
              </Button>
            </DialogFooter>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
