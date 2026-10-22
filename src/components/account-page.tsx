"use client";

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCheck,
  LuCopy,
  LuCrown,
  LuHardDrive,
  LuKey,
  LuLaptop,
  LuLogOut,
  LuRefreshCw,
  LuShieldCheck,
  LuUser,
} from "react-icons/lu";
import { LoadingButton } from "@/components/loading-button";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useCloudAuth } from "@/hooks/use-cloud-auth";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { RippleButton } from "./ui/ripple";

interface AccountPageProps {
  isOpen: boolean;
  onClose: () => void;
  subPage?: boolean;
  onOpenSignIn: () => void;
}

function readActiveIdentifier(): string {
  try {
    const active = window.localStorage.getItem(
      "buglogin.auth.active_identifier",
    );
    if (active?.trim()) return active.trim();
    const rem = window.localStorage.getItem("buglogin.auth.remember-email.v1");
    if (rem?.trim()) return rem.trim();
  } catch {
    // fallback
  }
  return "";
}

export function AccountPage({
  isOpen,
  onClose,
  subPage,
  onOpenSignIn,
}: AccountPageProps) {
  const { t } = useTranslation();
  const { user, account, isLoggedIn, logout, refreshProfile } = useCloudAuth();
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [isLoggingOut, setIsLoggingOut] = useState(false);
  const [copiedId, setCopiedId] = useState(false);
  const [activeTab, setActiveTab] = useState("profile");

  const handleRefresh = async () => {
    setIsRefreshing(true);
    try {
      await refreshProfile();
      showSuccessToast(
        t("account.refreshed", {
          defaultValue: "Đã làm mới thông tin tài khoản",
        }),
      );
    } catch (e) {
      showErrorToast(String(e));
    } finally {
      setIsRefreshing(false);
    }
  };

  const handleLogout = async () => {
    setIsLoggingOut(true);
    try {
      try {
        window.localStorage.removeItem("buglogin.auth.active_identifier");
      } catch {
        // ignore
      }
      await logout();
      showSuccessToast(
        t("account.loggedOut", { defaultValue: "Đã đăng xuất" }),
      );
      onClose();
    } catch (e) {
      showErrorToast(String(e));
    } finally {
      setIsLoggingOut(false);
    }
  };

  const accountId = account?.accountId || user?.id || "";
  const activeIdentifier = readActiveIdentifier();
  const actualEmailOrUser =
    user?.email || activeIdentifier || account?.accountId || "User";
  const plan = user?.plan ? user.plan.toUpperCase() : "STANDARD";
  const role = user?.teamRole
    ? user.teamRole.charAt(0).toUpperCase() + user.teamRole.slice(1)
    : "Member";

  const handleCopyAccountId = async () => {
    try {
      await navigator.clipboard.writeText(accountId);
      setCopiedId(true);
      showSuccessToast(
        t("account.idCopied", {
          defaultValue: "Đã sao chép Account ID vào bộ nhớ tạm",
        }),
      );
      setTimeout(() => setCopiedId(false), 2000);
    } catch (err) {
      showErrorToast(String(err));
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={onClose} subPage={subPage}>
      <DialogContent
        className={cn(
          "max-w-2xl p-6",
          subPage && "border-0 shadow-none bg-transparent",
        )}
      >
        <DialogHeader className="border-b border-border/50 pb-4">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3">
              <div className="grid size-11 place-items-center rounded-xl bg-primary/10 text-primary shadow-2xs">
                <LuUser className="size-5" />
              </div>
              <div>
                <DialogTitle className="text-base font-semibold">
                  {t("account.title", {
                    defaultValue: "Tài Khoản & Định Danh",
                  })}
                </DialogTitle>
                <p className="text-xs text-muted-foreground mt-0.5">
                  {t("account.subtitle", {
                    defaultValue:
                      "Quản lý thông tin định danh người dùng, phiên bảo mật và bảo vệ dữ liệu",
                  })}
                </p>
              </div>
            </div>
            {isLoggedIn && (
              <Badge
                variant="outline"
                className="gap-1.5 border-primary/30 bg-primary/10 px-3 py-1 text-xs text-primary font-medium"
              >
                <LuCrown className="size-3.5 text-amber-500" />
                <span>{plan}</span>
              </Badge>
            )}
          </div>
        </DialogHeader>

        <div className="space-y-4 py-2 text-xs">
          {/* User info card */}
          <div className="rounded-xl border border-border/70 bg-card p-4 space-y-3 shadow-2xs">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <div className="flex size-11 items-center justify-center rounded-full bg-primary/15 font-bold text-primary text-sm shadow-2xs">
                  {actualEmailOrUser[0]?.toUpperCase() || "A"}
                </div>
                <div className="space-y-0.5">
                  <div className="flex items-center gap-2">
                    <span className="font-semibold text-foreground text-sm">
                      {actualEmailOrUser}
                    </span>
                    <span className="inline-flex items-center gap-1 text-[11px] font-medium text-emerald-500">
                      <LuShieldCheck className="size-3" />
                      <span>{role}</span>
                    </span>
                  </div>
                  <div className="flex items-center gap-2 text-muted-foreground text-[11px]">
                    <span className="font-mono text-muted-foreground/90">
                      ID:{" "}
                      {accountId.length > 28
                        ? `${accountId.slice(0, 24)}...`
                        : accountId}
                    </span>
                  </div>
                </div>
              </div>

              <div className="flex items-center gap-1.5">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void handleCopyAccountId()}
                  className="h-8 px-2.5 text-xs gap-1.5 cursor-pointer"
                  title={t("account.copyId", {
                    defaultValue: "Sao chép Account ID",
                  })}
                >
                  {copiedId ? (
                    <LuCheck className="size-3 text-emerald-500" />
                  ) : (
                    <LuCopy className="size-3" />
                  )}
                  <span>
                    {copiedId
                      ? t("common.copied", { defaultValue: "Đã chép" })
                      : t("account.copyIdBtn", { defaultValue: "Copy ID" })}
                  </span>
                </Button>

                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void handleRefresh()}
                  disabled={isRefreshing}
                  className="h-8 px-2.5 text-xs gap-1.5 cursor-pointer"
                >
                  <LuRefreshCw
                    className={cn("size-3", isRefreshing && "animate-spin")}
                  />
                  <span>
                    {t("common.buttons.refresh", { defaultValue: "Làm mới" })}
                  </span>
                </Button>
              </div>
            </div>
          </div>

          {/* Tabbed details */}
          <Tabs
            value={activeTab}
            onValueChange={setActiveTab}
            className="w-full"
          >
            <TabsList className="grid grid-cols-2 h-9 p-0.5 bg-muted/50 rounded-lg">
              <TabsTrigger
                value="profile"
                className="gap-2 text-xs py-1.5 data-[state=active]:bg-background data-[state=active]:shadow-2xs font-medium"
              >
                <LuUser className="size-3.5" />
                <span>
                  {t("account.tabProfile", {
                    defaultValue: "Hồ Sơ & Hệ Thống",
                  })}
                </span>
              </TabsTrigger>
              <TabsTrigger
                value="security"
                className="gap-2 text-xs py-1.5 data-[state=active]:bg-background data-[state=active]:shadow-2xs font-medium"
              >
                <LuKey className="size-3.5" />
                <span>
                  {t("account.tabSecurity", {
                    defaultValue: "Bảo Mật & Phiên",
                  })}
                </span>
              </TabsTrigger>
            </TabsList>

            {/* TAB 1: Profile & System Info (Real data only, no mock quota bars) */}
            <TabsContent value="profile" className="space-y-3 mt-3">
              <div className="grid grid-cols-2 gap-3">
                <div className="rounded-xl border border-border/60 bg-muted/15 p-3.5 space-y-2">
                  <div className="flex items-center gap-2 font-medium text-foreground text-xs">
                    <LuShieldCheck className="size-4 text-emerald-500" />
                    <span>Bảo Mật Dữ Liệu</span>
                  </div>
                  <div className="text-[11px] text-muted-foreground space-y-1">
                    <p>• Mã hóa E2EE Cloud S3 Vault độc lập</p>
                    <p>• Khóa phần cứng bảo vệ cookies và storage</p>
                    <p>
                      • Trạng thái:{" "}
                      <strong className="text-emerald-500 font-semibold">
                        Được bảo vệ
                      </strong>
                    </p>
                  </div>
                </div>

                <div className="rounded-xl border border-border/60 bg-muted/15 p-3.5 space-y-2">
                  <div className="flex items-center gap-2 font-medium text-foreground text-xs">
                    <LuHardDrive className="size-4 text-primary" />
                    <span>Nhân Trình Duyệt</span>
                  </div>
                  <div className="text-[11px] text-muted-foreground space-y-1">
                    <p>
                      • Antidetect Engine:{" "}
                      <strong>Chromium Hardware Core</strong>
                    </p>
                    <p>• Fingerprint: Canvas, WebGL, WebRTC Masking</p>
                    <p>• Chế độ: Cách ly tuyệt đối giữa các Profile</p>
                  </div>
                </div>
              </div>

              <div className="rounded-xl border border-border/60 bg-muted/10 p-3.5 flex items-center justify-between">
                <div className="space-y-0.5">
                  <div className="text-xs font-semibold text-foreground">
                    Không Gian Hoạt Động (Workspace Active)
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    Quản lý thành viên, nhóm và phân bổ quyền qua tab Quản lý
                    Workspace
                  </div>
                </div>
                <Badge
                  variant="outline"
                  className="border-border/80 bg-background text-xs font-mono"
                >
                  Active
                </Badge>
              </div>
            </TabsContent>

            {/* TAB 2: Security & Session Status */}
            <TabsContent value="security" className="space-y-3 mt-3">
              <div className="rounded-xl border border-border/60 bg-muted/15 p-3.5 space-y-2">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2.5">
                    <div className="grid size-8 place-items-center rounded-lg bg-emerald-500/10 text-emerald-500">
                      <LuLaptop className="size-4" />
                    </div>
                    <div>
                      <div className="flex items-center gap-2">
                        <span className="font-semibold text-foreground text-xs">
                          Thiết bị Desktop Client
                        </span>
                        <Badge
                          variant="outline"
                          className="text-[10px] text-emerald-600 border-emerald-500/30"
                        >
                          Phiên hiện tại
                        </Badge>
                      </div>
                      <p className="text-[11px] text-muted-foreground mt-0.5">
                        Xác thực mật mã cục bộ • Phiên hoạt động an toàn
                      </p>
                    </div>
                  </div>
                </div>
              </div>

              <div className="rounded-xl border border-border/60 bg-card p-3.5 space-y-3">
                <div className="flex items-center gap-2 text-xs font-semibold text-foreground">
                  <LuKey className="size-4 text-primary" />
                  <span>Chính sách duy trì phiên (Session Persistence)</span>
                </div>
                <p className="text-[11px] text-muted-foreground leading-relaxed">
                  BugLogin mặc định giữ phiên đăng nhập hoạt động liên tục trong
                  suốt quá trình bạn sử dụng máy trạm. Tài khoản chỉ đăng xuất
                  khi bạn chủ động bấm nút Đăng xuất hoặc bị quản trị viên thu
                  hồi quyền từ xa.
                </p>
              </div>
            </TabsContent>
          </Tabs>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between border-t border-border/50 pt-3 mt-2">
          {isLoggedIn ? (
            <LoadingButton
              variant="destructive"
              size="sm"
              isLoading={isLoggingOut}
              onClick={() => void handleLogout()}
              className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
            >
              <LuLogOut className="size-3" />
              <span>
                {t("common.buttons.logout", {
                  defaultValue: "Đăng xuất tài khoản",
                })}
              </span>
            </LoadingButton>
          ) : (
            <Button
              size="sm"
              onClick={onOpenSignIn}
              className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
            >
              <LuKey className="size-3" />
              <span>
                {t("common.buttons.signIn", { defaultValue: "Đăng nhập" })}
              </span>
            </Button>
          )}

          {/* ONLY render close button in dialog/modal mode, NEVER in subPage mode */}
          {!subPage && (
            <RippleButton
              variant="outline"
              size="sm"
              onClick={onClose}
              className="h-8 text-xs font-medium cursor-pointer"
            >
              {t("common.buttons.close", { defaultValue: "Đóng" })}
            </RippleButton>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
