"use client";

import { invoke } from "@tauri-apps/api/core";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
  LuBan,
  LuCheck,
  LuClock,
  LuCopy,
  LuExternalLink,
  LuInfo,
  LuLink,
  LuMaximize2,
  LuMinimize2,
  LuPlay,
  LuSearch,
  LuShield,
  LuShieldAlert,
  LuShieldCheck,
  LuTrash2,
  LuUser,
  LuUserCheck,
  LuUserPlus,
  LuUsers,
} from "react-icons/lu";
import { LoadingButton } from "@/components/loading-button";
import { ScopedAccountCreationPage } from "@/components/scoped-account-creation-page";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { getActiveUserEmail } from "@/lib/identity-display";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import {
  analyzeBulkAccountRows,
  analyzeBulkInvitationEmails,
} from "../../apps/desktop/src/member-invitation-input";
import type {
  Member,
  MemberRole,
  MemberStatus,
} from "../../frontend/src/generated/command-registry";
import { RippleButton } from "./ui/ripple";

interface MemberManagementDialogProps {
  mode?: "page" | "dialog";
  isOpen: boolean;
  onClose: () => void;
  workspaceId: string;
  workspaceName?: string;
  organizationName?: string;
  onInvitePage?: () => void;
}

const desktopApi = createTauriDesktopApi();

export const ROLE_RANKS: Record<string, number> = {
  owner: 4,
  admin: 3,
  lead: 2,
  operator: 1,
  member: 0,
};

export function MemberManagementDialog({
  mode = "dialog",
  isOpen,
  onClose,
  workspaceId: propWorkspaceId,
  workspaceName = "Workspace Hiện Tại",
  organizationName,
  onInvitePage,
}: MemberManagementDialogProps) {
  const { t } = useTranslation();
  const [workspaceId, setWorkspaceId] = useState(propWorkspaceId || "");
  const [members, setMembers] = useState<Member[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [roleFilter, setRoleFilter] = useState<string>("all");
  const [isMaximized, setIsMaximized] = useState(false);
  const [accountCreationMode, setAccountCreationMode] = useState<
    "single" | "bulk" | null
  >(null);

  const [currentUser, setCurrentUser] = useState<{
    id: string;
    accountId: string;
    email: string;
    role: string;
    platformAdmin: boolean;
    organizationOwner: boolean;
  }>({
    id: "",
    accountId: "",
    email: "",
    role: "member",
    platformAdmin: false,
    organizationOwner: false,
  });

  // Invite modal state
  const [inviteModalOpen, setInviteModalOpen] = useState(false);
  const [inviteEmail, setInviteEmail] = useState("");
  const [inviteUsername, setInviteUsername] = useState("");
  const [createInviteAccount, setCreateInviteAccount] = useState(false);
  const [inviteError, setInviteError] = useState<string | null>(null);
  const [inviteRole, setInviteRole] = useState<MemberRole>("operator");
  const [isInviting, setIsInviting] = useState(false);
  const [generatedInvite, setGeneratedInvite] = useState<{
    code: string;
    deepLink: string;
    expiresAt: string;
    createdPendingAccount?: boolean;
  } | null>(null);
  const [copiedCode, setCopiedCode] = useState(false);
  const [copiedLink, setCopiedLink] = useState(false);

  // Status transition state
  const [updatingMemberId, setUpdatingMemberId] = useState<string | null>(null);

  // Offboarding / Revocation state (Section 13.5)
  const [offboardMember, setOffboardMember] = useState<Member | null>(null);
  const [isOffboarding, setIsOffboarding] = useState(false);

  // Multi-select for bulk actions
  const [selectedMemberIds, setSelectedMemberIds] = useState<Set<string>>(
    new Set(),
  );
  const [isBulkExecuting, setIsBulkExecuting] = useState(false);
  const [bulkActionError, setBulkActionError] = useState<string | null>(null);

  // Bulk Provision Modal state
  const [bulkModalOpen, setBulkModalOpen] = useState(false);
  const [bulkEmailsText, setBulkEmailsText] = useState("");
  const [bulkCreateAccounts, setBulkCreateAccounts] = useState(false);
  const [bulkInputError, setBulkInputError] = useState<string | null>(null);
  const bulkEmailAnalysis = useMemo(
    () => analyzeBulkInvitationEmails(bulkEmailsText),
    [bulkEmailsText],
  );
  const bulkAccountAnalysis = useMemo(
    () => analyzeBulkAccountRows(bulkEmailsText),
    [bulkEmailsText],
  );
  const [bulkRole, setBulkRole] = useState<MemberRole>("operator");
  const [isBulkProcessing, setIsBulkProcessing] = useState(false);
  const [bulkProgress, setBulkProgress] = useState<{
    current: number;
    total: number;
  }>({ current: 0, total: 0 });
  const [bulkResults, setBulkResults] = useState<Array<{
    email: string;
    username?: string;
    accountCreated?: boolean;
    inviteCode?: string;
    deepLink?: string;
    status: "success" | "error";
    error?: string;
  }> | null>(null);
  const [bulkCopiedAll, setBulkCopiedAll] = useState(false);
  const bulkSuccessCount =
    bulkResults?.filter((result) => result.status === "success").length ?? 0;
  const bulkFailureCount =
    bulkResults?.filter((result) => result.status === "error").length ?? 0;

  const currentUserRank = currentUser.platformAdmin
    ? Number.POSITIVE_INFINITY
    : currentUser.organizationOwner
      ? ROLE_RANKS.owner
      : (ROLE_RANKS[currentUser.role] ?? 0);
  const assignableRoles: MemberRole[] = (
    ["owner", "admin", "lead", "operator", "member"] as MemberRole[]
  ).filter((r) => (ROLE_RANKS[r] ?? 0) < currentUserRank);

  // Resolve workspace if needed
  useEffect(() => {
    if (propWorkspaceId) {
      setWorkspaceId(propWorkspaceId);
      return;
    }
    let disposed = false;
    void invoke<Array<{ id: string; name: string; selected?: boolean }>>(
      "get_available_workspaces",
    )
      .then((workspaces) => {
        if (disposed) return;
        const active = workspaces.find((w) => w.selected) || workspaces[0];
        if (active) setWorkspaceId(active.id);
      })
      .catch((err) =>
        console.error("Failed to load workspace for members:", err),
      );
    return () => {
      disposed = true;
    };
  }, [propWorkspaceId]);

  const loadMembers = useCallback(async () => {
    if (!workspaceId) return false;
    setIsLoading(true);
    try {
      const [page, account] = await Promise.all([
        desktopApi.listMembers(workspaceId),
        desktopApi.getAccount(),
      ]);
      setMembers(page.items);
      if (account?.accountId) {
        const membership = page.items.find(
          (member) => member.accountId === account.accountId,
        );
        const activeEmail = getActiveUserEmail();
        setCurrentUser({
          id: membership?.id ?? account.accountId,
          accountId: account.accountId,
          email: membership?.identity?.email ?? activeEmail,
          role: membership?.role ?? "member",
          platformAdmin: account.platformAdmin,
          organizationOwner: account.managedWorkspaceIds.includes(workspaceId),
        });
      }
      return true;
    } catch (err) {
      console.error("Failed to load members:", err);
      showErrorToast(
        t("members.loadFailed", {
          defaultValue: "Không thể tải danh sách thành viên",
        }),
      );
      return false;
    } finally {
      setIsLoading(false);
    }
  }, [workspaceId, t]);

  useEffect(() => {
    if (isOpen) {
      void loadMembers();
    }
  }, [isOpen, loadMembers]);

  const handleRoleChange = async (memberId: string, newRole: MemberRole) => {
    if (!workspaceId) return;
    setUpdatingMemberId(memberId);
    try {
      await desktopApi.updateMemberRole({
        workspaceId,
        memberId,
        role: newRole,
      });
      showSuccessToast(
        t("members.roleUpdated", {
          defaultValue: "Đã cập nhật vai trò thành viên",
        }),
      );
      void loadMembers();
    } catch (err) {
      console.error("Failed to update role:", err);
      showErrorToast(String(err));
    } finally {
      setUpdatingMemberId(null);
    }
  };

  const handleToggleSuspend = async (member: Member) => {
    if (!workspaceId) return;
    setUpdatingMemberId(member.id);
    try {
      if (member.status === "suspended") {
        await desktopApi.activateMember({ workspaceId, memberId: member.id });
        showSuccessToast(
          t("members.activated", {
            defaultValue: "Đã kích hoạt lại thành viên",
          }),
        );
      } else {
        await desktopApi.suspendMember({ workspaceId, memberId: member.id });
        showSuccessToast(
          t("members.suspended", {
            defaultValue: "Đã tạm khóa tài khoản thành viên",
          }),
        );
      }
      void loadMembers();
    } catch (err) {
      console.error("Failed to change member status:", err);
      showErrorToast(String(err));
    } finally {
      setUpdatingMemberId(null);
    }
  };

  const handleCreateInvite = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!inviteEmail.trim() || !workspaceId) return;

    setIsInviting(true);
    setInviteError(null);
    try {
      const expiresAt = Math.floor(Date.now() / 1000) + 7 * 24 * 60 * 60;
      const invitation = await desktopApi.createMemberInvitation({
        workspaceId,
        email: inviteEmail.trim(),
        role: inviteRole,
        expiresAt,
      });
      if (!invitation.code) {
        throw new Error(
          "Lời mời đã tồn tại nhưng mã chỉ hiển thị một lần. Hãy thu hồi lời mời cũ rồi tạo lời mời mới.",
        );
      }
      setGeneratedInvite({
        code: `${invitation.invitationId}.${invitation.code}`,
        deepLink: `buglogin://invite?invitationId=${encodeURIComponent(invitation.invitationId)}&code=${encodeURIComponent(invitation.code)}`,
        expiresAt: new Date(invitation.expiresAt * 1000).toLocaleString(),
      });
      showSuccessToast(
        t("members.inviteCreated", {
          defaultValue: "Đã tạo lời mời thành viên thành công",
        }),
      );
    } catch (err) {
      console.error("Failed to invite member:", err);
      const detail = err instanceof Error ? err.message : JSON.stringify(err);
      setInviteError(detail);
      showErrorToast(detail);
    } finally {
      setIsInviting(false);
    }
  };

  const handleCopyCode = async () => {
    if (!generatedInvite) return;
    try {
      await navigator.clipboard.writeText(generatedInvite.code);
      setCopiedCode(true);
      showSuccessToast("Đã sao chép mã mời vào bộ nhớ tạm");
      setTimeout(() => setCopiedCode(false), 2000);
    } catch (err) {
      showErrorToast(String(err));
    }
  };

  const handleCopyLink = async () => {
    if (!generatedInvite) return;
    try {
      await navigator.clipboard.writeText(generatedInvite.deepLink);
      setCopiedLink(true);
      showSuccessToast("Đã sao chép liên kết mời vào bộ nhớ tạm");
      setTimeout(() => setCopiedLink(false), 2000);
    } catch (err) {
      showErrorToast(String(err));
    }
  };

  const handleOpenOffboard = (member: Member) => {
    setOffboardMember(member);
  };

  const handleExecuteOffboarding = async () => {
    if (!offboardMember || !workspaceId) return;
    setIsOffboarding(true);
    try {
      await desktopApi.suspendMember({
        workspaceId,
        memberId: offboardMember.id,
      });
      const displayName = offboardMember.identity?.username ?? "thành viên";
      showSuccessToast(`Đã tạm ngưng quyền thành viên của ${displayName}.`);
      setOffboardMember(null);
      void loadMembers();
    } catch (err) {
      console.error("Failed to offboard member:", err);
      showErrorToast(String(err));
    } finally {
      setIsOffboarding(false);
    }
  };

  const filteredMembers = useMemo(() => {
    let list = members;
    if (roleFilter !== "all") {
      list = list.filter((m) => m.role === roleFilter);
    }
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      list = list.filter((m) =>
        [m.identity?.username, m.identity?.email, m.accountId, m.id]
          .filter(Boolean)
          .some((value) => value?.toLowerCase().includes(q)),
      );
    }
    return list;
  }, [members, roleFilter, searchQuery]);

  const manageableMembers = useMemo(() => {
    return filteredMembers.filter((m) => {
      const memberRank = ROLE_RANKS[m.role] ?? 0;
      const isSelf =
        m.id === currentUser.id || m.accountId === currentUser.accountId;
      return !isSelf && memberRank < currentUserRank;
    });
  }, [filteredMembers, currentUser, currentUserRank]);

  const handleToggleSelectMember = (id: string) => {
    setSelectedMemberIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleSelectAllManageable = () => {
    if (
      selectedMemberIds.size === manageableMembers.length &&
      manageableMembers.length > 0
    ) {
      setSelectedMemberIds(new Set());
    } else {
      setSelectedMemberIds(new Set(manageableMembers.map((m) => m.id)));
    }
  };

  const handleBulkChangeRole = async (newRole: MemberRole) => {
    if (!workspaceId || selectedMemberIds.size === 0) return;
    setIsBulkExecuting(true);
    setBulkActionError(null);
    const failedIds: string[] = [];
    try {
      for (const memberId of selectedMemberIds) {
        try {
          await desktopApi.updateMemberRole({
            workspaceId,
            memberId,
            role: newRole,
          });
        } catch {
          failedIds.push(memberId);
        }
      }
      const refreshed = await loadMembers();
      setSelectedMemberIds(
        new Set(refreshed ? failedIds : [...selectedMemberIds]),
      );
      if (failedIds.length > 0 || !refreshed) {
        const summary = !refreshed
          ? "Đã gửi thay đổi nhưng không làm mới được danh sách. Hãy tải lại trước khi thao tác tiếp."
          : `${selectedMemberIds.size - failedIds.length} thành công, ${failedIds.length} thất bại. Các thành viên lỗi vẫn được chọn để thử lại.`;
        setBulkActionError(summary);
        showErrorToast(summary);
      } else {
        showSuccessToast(
          t("members.bulkActions.roleUpdated", {
            count: selectedMemberIds.size,
          }),
        );
      }
    } finally {
      setIsBulkExecuting(false);
    }
  };

  const handleBulkSuspend = async () => {
    if (!workspaceId || selectedMemberIds.size === 0) return;
    setIsBulkExecuting(true);
    setBulkActionError(null);
    const failedIds: string[] = [];
    try {
      for (const memberId of selectedMemberIds) {
        try {
          await desktopApi.suspendMember({ workspaceId, memberId });
        } catch {
          failedIds.push(memberId);
        }
      }
      const refreshed = await loadMembers();
      setSelectedMemberIds(
        new Set(refreshed ? failedIds : [...selectedMemberIds]),
      );
      if (failedIds.length > 0 || !refreshed) {
        const summary = !refreshed
          ? "Đã gửi thay đổi nhưng không làm mới được danh sách. Hãy tải lại trước khi thao tác tiếp."
          : `${selectedMemberIds.size - failedIds.length} thành công, ${failedIds.length} thất bại. Các thành viên lỗi vẫn được chọn để thử lại.`;
        setBulkActionError(summary);
        showErrorToast(summary);
      } else {
        showSuccessToast(
          t("members.bulkActions.suspendedCount", {
            count: selectedMemberIds.size,
          }),
        );
      }
    } finally {
      setIsBulkExecuting(false);
    }
  };

  const handleBulkActivate = async () => {
    if (!workspaceId || selectedMemberIds.size === 0) return;
    setIsBulkExecuting(true);
    setBulkActionError(null);
    const failedIds: string[] = [];
    try {
      for (const memberId of selectedMemberIds) {
        try {
          await desktopApi.activateMember({ workspaceId, memberId });
        } catch {
          failedIds.push(memberId);
        }
      }
      const refreshed = await loadMembers();
      setSelectedMemberIds(
        new Set(refreshed ? failedIds : [...selectedMemberIds]),
      );
      if (failedIds.length > 0 || !refreshed) {
        const summary = !refreshed
          ? "Đã gửi thay đổi nhưng không làm mới được danh sách. Hãy tải lại trước khi thao tác tiếp."
          : `${selectedMemberIds.size - failedIds.length} thành công, ${failedIds.length} thất bại. Các thành viên lỗi vẫn được chọn để thử lại.`;
        setBulkActionError(summary);
        showErrorToast(summary);
      } else {
        showSuccessToast(
          t("members.bulkActions.activatedCount", {
            count: selectedMemberIds.size,
          }),
        );
      }
    } finally {
      setIsBulkExecuting(false);
    }
  };

  const handleCreateBulkInvitations = async () => {
    setBulkInputError(null);
    const entryCount = bulkCreateAccounts
      ? bulkAccountAnalysis.entryCount
      : bulkEmailAnalysis.entryCount;
    if (!workspaceId || entryCount === 0) return;
    if (
      bulkCreateAccounts &&
      (bulkAccountAnalysis.invalidLines.length > 0 ||
        bulkAccountAnalysis.duplicateLines.length > 0 ||
        bulkAccountAnalysis.exceedsLimit)
    ) {
      setBulkInputError(
        `Kiểm tra dòng không hợp lệ: ${bulkAccountAnalysis.invalidLines.join(", ") || "—"}; trùng: ${bulkAccountAnalysis.duplicateLines.join(", ") || "—"}; tối đa 100 dòng.`,
      );
      return;
    }
    if (
      !bulkCreateAccounts &&
      (bulkEmailAnalysis.invalid.length > 0 ||
        bulkEmailAnalysis.duplicates.length > 0 ||
        bulkEmailAnalysis.exceedsLimit)
    ) {
      const reasons = [
        bulkEmailAnalysis.invalid.length > 0 &&
          `${bulkEmailAnalysis.invalid.length} email không hợp lệ`,
        bulkEmailAnalysis.duplicates.length > 0 &&
          `${bulkEmailAnalysis.duplicates.length} email bị trùng`,
        bulkEmailAnalysis.exceedsLimit && "tối đa 100 email mỗi lần",
      ].filter(Boolean);
      setBulkInputError(`Chưa tạo lời mời: ${reasons.join("; ")}.`);
      return;
    }

    const entries = bulkCreateAccounts
      ? bulkAccountAnalysis.rows
      : bulkEmailAnalysis.emails.map((email, index) => ({
          line: index + 1,
          email,
          username: undefined,
        }));
    setIsBulkProcessing(true);
    setBulkProgress({ current: 0, total: entries.length });
    const results: Array<{
      email: string;
      username?: string;
      accountCreated?: boolean;
      inviteCode?: string;
      deepLink?: string;
      status: "success" | "error";
      error?: string;
    }> = [];

    for (let i = 0; i < entries.length; i++) {
      const { email, username } = entries[i];
      setBulkProgress({
        current: i + 1,
        total: entries.length,
      });

      try {
        const invitation = await desktopApi.createMemberInvitation({
          workspaceId,
          email,
          role: bulkRole,
          expiresAt: Math.floor(Date.now() / 1000) + 7 * 24 * 60 * 60,
        });
        if (!invitation.code) {
          throw new Error("Lời mời đã tồn tại; mã không thể hiển thị lại.");
        }
        const inviteCode = `${invitation.invitationId}.${invitation.code}`;
        const deepLink = `buglogin://invite?invitationId=${encodeURIComponent(invitation.invitationId)}&code=${encodeURIComponent(invitation.code)}`;
        results.push({
          email,
          username,
          inviteCode,
          deepLink,
          status: "success",
        });
      } catch (err) {
        const detail =
          err instanceof Error ? err.message : "Không thể tạo lời mời.";
        results.push({
          email,
          username,
          status: "error",
          error: detail,
        });
      }
    }

    setBulkResults(results);
    setIsBulkProcessing(false);
  };

  const handleCopyBulkResults = async () => {
    if (!bulkResults) return;
    const lines = bulkResults.map((r) => {
      if (r.status === "error") return `${r.email}\tError: ${r.error}`;
      if (bulkCreateAccounts)
        return `${r.username}\t${r.email}\t${bulkRole}\tTạo account; email chờ xác minh`;
      return `${r.email}\tCode: ${r.inviteCode}\tLink: ${r.deepLink}`;
    });
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      setBulkCopiedAll(true);
      showSuccessToast(
        t("members.bulkModal.copied", { defaultValue: "Đã sao chép!" }),
      );
      setTimeout(() => setBulkCopiedAll(false), 2000);
    } catch (e) {
      showErrorToast(String(e));
    }
  };

  const handleExportBulkCsv = () => {
    if (!bulkResults) return;
    const header = bulkCreateAccounts
      ? "Username,Email,Role,Status"
      : "Email,InviteCode,DeepLink,Role,Status";
    const rows = bulkResults.map((r) => {
      return bulkCreateAccounts
        ? `"${r.username || ""}","${r.email}","${bulkRole}","${r.status === "success" ? "created_pending_verification" : r.error || "error"}"`
        : `"${r.email}","${r.inviteCode || ""}","${r.deepLink || ""}","${bulkRole}","${r.status}"`;
    });
    const csvContent = [header, ...rows].join("\n");
    const encodedUri = `data:text/csv;charset=utf-8,%EF%BB%BF${encodeURIComponent(csvContent)}`;
    const link = document.createElement("a");
    link.setAttribute("href", encodedUri);
    link.setAttribute(
      "download",
      `buglogin-${bulkCreateAccounts ? "accounts" : "invitations"}-${new Date().toISOString().slice(0, 10)}.csv`,
    );
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  };

  const getStatusBadge = (status: MemberStatus) => {
    switch (status) {
      case "active":
        return (
          <Badge className="bg-emerald-500/10 text-emerald-600 border-emerald-500/30 text-[10px] gap-1 font-semibold">
            <span className="size-1.5 rounded-full bg-emerald-500 animate-pulse" />
            <span>Active</span>
          </Badge>
        );
      case "invited":
        return (
          <Badge
            variant="outline"
            className="bg-amber-500/10 text-amber-600 border-amber-500/30 text-[10px] gap-1 font-semibold"
          >
            <LuClock className="size-3" />
            <span>Invited</span>
          </Badge>
        );
      case "suspended":
        return (
          <Badge
            variant="destructive"
            className="text-[10px] gap-1 font-semibold"
          >
            <LuShieldAlert className="size-3" />
            <span>Suspended</span>
          </Badge>
        );
    }
  };

  const MemberFrame = ({ children }: { children: ReactNode }) =>
    mode === "page" ? (
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto bg-background">
        {children}
      </div>
    ) : (
      <Dialog open={isOpen} onOpenChange={onClose}>
        <DialogContent
          className={cn(
            "p-6 transition-all duration-200 overflow-y-auto flex flex-col",
            isMaximized
              ? "!fixed !inset-2 !max-w-none !w-[calc(100vw-1rem)] !h-[calc(100vh-1rem)] !max-h-none !translate-x-0 !translate-y-0 !top-2 !left-2 rounded-2xl"
              : "max-w-4xl max-h-[88vh]",
          )}
        >
          {children}
        </DialogContent>
      </Dialog>
    );
  const workspaceLabel = organizationName
    ? `${organizationName} / ${workspaceName}`
    : workspaceName;

  if (accountCreationMode) {
    return (
      <ScopedAccountCreationPage
        workspaceId={workspaceId}
        workspaceName={workspaceName}
        organizationName={organizationName}
        initialMode={accountCreationMode}
        onBack={() => setAccountCreationMode(null)}
        onCreated={() => void loadMembers()}
      />
    );
  }

  return (
    <>
      <MemberFrame>
        <DialogHeader className="border-b border-border/50 pb-3">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3">
              {mode === "page" && (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  onClick={onClose}
                  aria-label="Quay lại workspace"
                >
                  <LuArrowLeft className="size-4" />
                </Button>
              )}
              <div className="grid size-10 place-items-center rounded-xl bg-primary/10 text-primary shadow-xs shrink-0">
                <LuUsers className="size-5" />
              </div>
              <div>
                {mode === "page" ? (
                  <h1 className="text-base font-semibold">
                    {t("members.title", {
                      defaultValue: "Quản Lý Thành Viên & Phân Quyền",
                    })}
                  </h1>
                ) : (
                  <DialogTitle className="text-base font-semibold">
                    {t("members.title", {
                      defaultValue: "Quản Lý Thành Viên & Phân Quyền",
                    })}
                  </DialogTitle>
                )}
                {mode === "page" ? (
                  <p className="text-xs text-muted-foreground mt-0.5">
                    {t("members.subtitle", {
                      workspaceName: workspaceLabel,
                      used: members.length,
                      max: 20,
                      defaultValue: `Không gian: ${workspaceLabel} · Đã dùng: ${members.length} / 20 ghế thành viên`,
                    })}
                  </p>
                ) : (
                  <DialogDescription className="text-xs text-muted-foreground mt-0.5">
                    {t("members.subtitle", {
                      workspaceName: workspaceLabel,
                      used: members.length,
                      max: 20,
                      defaultValue: `Không gian: ${workspaceLabel} · Đã dùng: ${members.length} / 20 ghế thành viên`,
                    })}
                  </DialogDescription>
                )}
              </div>
            </div>

            {/* Maximize / Minimize Fullscreen Toggle */}
            {mode === "dialog" && (
              <div className="flex items-center gap-2 pr-6">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setIsMaximized(!isMaximized)}
                  className="h-8 px-2.5 text-xs gap-1.5 text-muted-foreground hover:text-foreground cursor-pointer"
                  title={
                    isMaximized
                      ? "Thu nhỏ về kích thước chuẩn"
                      : "Mở rộng toàn màn hình"
                  }
                >
                  {isMaximized ? (
                    <>
                      <LuMinimize2 className="size-3.5 text-primary" />
                      <span className="hidden sm:inline">Thu nhỏ</span>
                    </>
                  ) : (
                    <>
                      <LuMaximize2 className="size-3.5 text-primary" />
                      <span className="hidden sm:inline">Toàn màn hình</span>
                    </>
                  )}
                </Button>
              </div>
            )}
          </div>
        </DialogHeader>

        <div className="space-y-3.5 py-1 text-xs">
          {/* Role & Access Status Banner */}
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-primary/20 bg-primary/5 p-2.5 text-xs">
            <div className="flex items-center gap-2">
              <LuShieldCheck className="size-4 text-primary shrink-0" />
              <div>
                <span className="text-muted-foreground">
                  {t("members.yourAccount", {
                    defaultValue: "Tài khoản của bạn:",
                  })}{" "}
                </span>
                <strong className="text-foreground">
                  {currentUser.email || "Đang đăng nhập"}
                </strong>
                <span className="mx-1.5 text-muted-foreground">•</span>
                <Badge
                  variant="outline"
                  className="text-[10px] font-mono border-primary/40 text-primary capitalize font-bold"
                >
                  {t(`roles.${currentUser.role}`, {
                    defaultValue: currentUser.role,
                  })}
                </Badge>
                <span className="text-muted-foreground ml-2 hidden sm:inline">
                  —{" "}
                  {t("members.canManageLowerRoles", {
                    defaultValue:
                      "Chỉ có thể quản lý & phân quyền cho vai trò cấp dưới",
                  })}
                </span>
              </div>
            </div>
          </div>

          {/* Toolbar: Search, Filter, Direct Provision, Bulk Provision, Invite */}
          <div className="flex flex-wrap items-center justify-between gap-2.5">
            <div className="flex items-center gap-2">
              <div className="relative w-48 sm:w-60">
                <LuSearch className="absolute left-2.5 top-2.5 size-3.5 text-muted-foreground" />
                <Input
                  placeholder={t("members.searchPlaceholder", {
                    defaultValue: "Tìm theo tên hoặc email...",
                  })}
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="h-8 pl-8 text-xs"
                />
              </div>
              <Select value={roleFilter} onValueChange={setRoleFilter}>
                <SelectTrigger className="h-8 w-36 text-xs">
                  <SelectValue placeholder="Lọc vai trò..." />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all" className="text-xs">
                    Tất cả vai trò
                  </SelectItem>
                  <SelectItem value="owner" className="text-xs">
                    Owner
                  </SelectItem>
                  <SelectItem value="admin" className="text-xs">
                    Admin
                  </SelectItem>
                  <SelectItem value="lead" className="text-xs">
                    Lead — quản lý thành viên và đội nhóm cấp thấp hơn
                  </SelectItem>
                  <SelectItem value="operator" className="text-xs">
                    Operator
                  </SelectItem>
                  <SelectItem value="member" className="text-xs">
                    Member
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="flex items-center gap-2">
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  setBulkCreateAccounts(false);
                  setBulkEmailsText("");
                  setBulkInputError(null);
                  setBulkRole(assignableRoles[0] || "operator");
                  setBulkResults(null);
                  setBulkModalOpen(true);
                }}
                disabled={currentUserRank <= 1}
                className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
              >
                <LuUsers className="size-3.5 text-primary" />
                <span>
                  {t("members.bulkProvisionButton", {
                    defaultValue: "Mời hàng loạt",
                  })}
                </span>
              </Button>

              <Button
                size="sm"
                variant="outline"
                onClick={() => setAccountCreationMode("bulk")}
                disabled={currentUserRank <= 1}
                className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
              >
                <LuUsers className="size-3.5" />
                <span>Tạo account hàng loạt</span>
              </Button>

              <Button
                size="sm"
                onClick={() => {
                  if (mode === "page" && onInvitePage) {
                    onInvitePage();
                    return;
                  }
                  setCreateInviteAccount(false);
                  setGeneratedInvite(null);
                  setInviteEmail("");
                  setInviteUsername("");
                  setInviteRole(assignableRoles[0] || "operator");
                  setInviteModalOpen(true);
                }}
                disabled={currentUserRank <= 1}
                className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
              >
                <LuUserPlus className="size-3.5" />
                <span>
                  {t("members.inviteButton", {
                    defaultValue: "Mời Thành Viên",
                  })}
                </span>
              </Button>

              <Button
                size="sm"
                onClick={() => setAccountCreationMode("single")}
                disabled={currentUserRank <= 1}
                className="h-8 gap-1.5 text-xs font-medium cursor-pointer"
              >
                <LuUserPlus className="size-3.5" />
                <span>Tạo tài khoản</span>
              </Button>
            </div>
          </div>

          {/* Members Table */}
          <div className="rounded-lg border border-border overflow-hidden">
            <div
              className={
                mode === "page"
                  ? "max-h-[calc(100vh-360px)] overflow-y-auto"
                  : "max-h-[260px] overflow-y-auto"
              }
            >
              <table className="w-full text-left text-xs">
                <thead className="bg-muted/50 border-b border-border sticky top-0 font-medium text-muted-foreground text-[11px]">
                  <tr>
                    <th className="py-2.5 px-3 w-8">
                      <Checkbox
                        checked={
                          manageableMembers.length > 0 &&
                          selectedMemberIds.size === manageableMembers.length
                        }
                        onCheckedChange={handleSelectAllManageable}
                        disabled={manageableMembers.length === 0}
                        aria-label="Select all manageable members"
                      />
                    </th>
                    <th className="py-2.5 px-3">Thành viên & Email</th>
                    <th className="py-2.5 px-3 w-36">Vai trò</th>
                    <th className="py-2.5 px-3 w-28">Trạng thái</th>
                    <th className="py-2.5 px-3 w-28 text-center">Thao tác</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border/60">
                  {isLoading ? (
                    <tr>
                      <td
                        colSpan={5}
                        className="py-8 text-center text-muted-foreground"
                      >
                        {t("common.buttons.loading", {
                          defaultValue: "Đang tải dữ liệu...",
                        })}
                      </td>
                    </tr>
                  ) : filteredMembers.length === 0 ? (
                    <tr>
                      <td
                        colSpan={5}
                        className="py-8 text-center text-muted-foreground"
                      >
                        {t("members.noMembersFound", {
                          defaultValue: "Không tìm thấy thành viên phù hợp",
                        })}
                      </td>
                    </tr>
                  ) : (
                    filteredMembers.map((member) => {
                      const memberRank = ROLE_RANKS[member.role] ?? 0;
                      const isSelf =
                        member.id === currentUser.id ||
                        member.accountId === currentUser.accountId;
                      const isHigherOrEqual = memberRank >= currentUserRank;
                      const canManage = !isSelf && !isHigherOrEqual;
                      const isUpdating = updatingMemberId === member.id;
                      const displayIdentity =
                        member.identity?.username ??
                        "Danh tính chưa được cấp quyền xem";
                      const displayEmail = member.identity?.email ?? "—";
                      const initials =
                        member.identity?.username?.slice(0, 2).toUpperCase() ??
                        "?";

                      return (
                        <tr
                          key={member.id}
                          className={cn(
                            "transition-colors hover:bg-muted/30",
                            member.status === "suspended" &&
                              "opacity-60 bg-muted/20",
                          )}
                        >
                          <td className="py-2.5 px-3 w-8">
                            <Checkbox
                              checked={selectedMemberIds.has(member.id)}
                              onCheckedChange={() =>
                                handleToggleSelectMember(member.id)
                              }
                              disabled={!canManage}
                              aria-label={`Select ${member.accountId}`}
                            />
                          </td>
                          <td className="py-2.5 px-3">
                            <div className="flex items-center gap-2.5">
                              <div className="flex size-7 items-center justify-center rounded-full bg-primary/10 text-primary font-bold text-xs">
                                {initials}
                              </div>
                              <div>
                                <div className="font-semibold text-foreground text-xs flex items-center gap-1.5">
                                  <span>{displayIdentity}</span>
                                  {isSelf && (
                                    <Badge
                                      variant="outline"
                                      className="text-[9px] py-0 h-4 border-primary/40 text-primary"
                                    >
                                      Bạn
                                    </Badge>
                                  )}
                                </div>
                                <div className="text-[11px] text-muted-foreground">
                                  {displayEmail}
                                </div>
                                <div className="text-[10px] text-muted-foreground/50 font-mono">
                                  {member.id}
                                </div>
                              </div>
                            </div>
                          </td>
                          <td className="py-2.5 px-3">
                            {isSelf ? (
                              <Badge className="bg-amber-500/15 text-amber-600 border-amber-300 text-[10px] gap-1 font-semibold">
                                <LuShieldCheck className="size-3" />
                                <span className="capitalize">
                                  {t(`roles.${member.role}`, {
                                    defaultValue: member.role,
                                  })}
                                </span>
                              </Badge>
                            ) : !canManage ? (
                              <Badge
                                variant="outline"
                                className="text-[10px] font-medium capitalize"
                              >
                                {t(`roles.${member.role}`, {
                                  defaultValue: member.role,
                                })}
                              </Badge>
                            ) : (
                              <Select
                                value={member.role}
                                onValueChange={(val) =>
                                  void handleRoleChange(
                                    member.id,
                                    val as MemberRole,
                                  )
                                }
                                disabled={isUpdating}
                              >
                                <SelectTrigger className="h-7 text-xs">
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  {assignableRoles.map((r) => (
                                    <SelectItem
                                      key={r}
                                      value={r}
                                      className="text-xs capitalize"
                                    >
                                      {t(`roles.${r}`, { defaultValue: r })}
                                    </SelectItem>
                                  ))}
                                </SelectContent>
                              </Select>
                            )}
                          </td>
                          <td className="py-2.5 px-3">
                            {getStatusBadge(member.status)}
                          </td>
                          <td className="py-2.5 px-3 text-right whitespace-nowrap">
                            {isSelf ? (
                              <span className="text-[11px] text-muted-foreground italic">
                                (Chính bạn)
                              </span>
                            ) : isHigherOrEqual ? (
                              <Badge
                                variant="outline"
                                className="text-[10px] text-muted-foreground border-border/80 font-normal"
                              >
                                {t("members.protectedRole", {
                                  defaultValue: "Vai trò được bảo vệ",
                                })}
                              </Badge>
                            ) : (
                              <div className="flex items-center justify-end gap-1.5">
                                <LoadingButton
                                  variant={
                                    member.status === "suspended"
                                      ? "outline"
                                      : "ghost"
                                  }
                                  size="sm"
                                  isLoading={isUpdating}
                                  onClick={() =>
                                    void handleToggleSuspend(member)
                                  }
                                  className={cn(
                                    "h-7 px-2 text-[10px] cursor-pointer font-medium",
                                    member.status === "suspended"
                                      ? "text-emerald-600 hover:text-emerald-700"
                                      : "text-muted-foreground hover:text-foreground",
                                  )}
                                >
                                  {member.status === "suspended" ? (
                                    <>
                                      <LuUserCheck className="size-3 mr-1 text-emerald-500" />
                                      <span>Mở khóa</span>
                                    </>
                                  ) : (
                                    <>
                                      <LuBan className="size-3 mr-1" />
                                      <span>Khóa</span>
                                    </>
                                  )}
                                </LoadingButton>
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="sm"
                                  onClick={() => handleOpenOffboard(member)}
                                  className="h-7 px-2 text-[10px] gap-1 cursor-pointer text-destructive hover:bg-destructive/10 font-medium"
                                >
                                  <LuTrash2 className="size-3" />
                                  <span>Thu hồi</span>
                                </Button>
                              </div>
                            )}
                          </td>
                        </tr>
                      );
                    })
                  )}
                </tbody>
              </table>
            </div>
          </div>

          {/* Floating Bulk Action Bar */}
          {selectedMemberIds.size > 0 && (
            <div className="flex flex-wrap items-center justify-between gap-2.5 rounded-lg border border-primary/40 bg-card p-2.5 shadow-md animate-in fade-in slide-in-from-bottom-2">
              <div className="flex items-center gap-2">
                <Badge variant="default" className="text-xs px-2.5 py-0.5">
                  {t("members.bulkActions.selected", {
                    count: selectedMemberIds.size,
                  })}
                </Badge>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedMemberIds(new Set())}
                  className="h-7 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  {t("members.bulkActions.clear", {
                    defaultValue: "Bỏ chọn",
                  })}
                </Button>
              </div>

              <div className="flex items-center gap-2">
                <Select
                  onValueChange={(role) =>
                    void handleBulkChangeRole(role as MemberRole)
                  }
                >
                  <SelectTrigger className="h-7 w-32 text-xs">
                    <SelectValue
                      placeholder={t("members.bulkActions.changeRole", {
                        defaultValue: "Đổi vai trò",
                      })}
                    />
                  </SelectTrigger>
                  <SelectContent>
                    {assignableRoles.map((r) => (
                      <SelectItem
                        key={r}
                        value={r}
                        className="text-xs capitalize"
                      >
                        {t(`roles.${r}`, { defaultValue: r })}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>

                <LoadingButton
                  variant="outline"
                  size="sm"
                  isLoading={isBulkExecuting}
                  onClick={() => void handleBulkSuspend()}
                  className="h-7 text-xs gap-1 cursor-pointer text-amber-600 hover:text-amber-700"
                >
                  <LuBan className="size-3" />
                  <span>
                    {t("members.bulkActions.suspend", {
                      defaultValue: "Khóa",
                    })}
                  </span>
                </LoadingButton>

                <LoadingButton
                  variant="outline"
                  size="sm"
                  isLoading={isBulkExecuting}
                  onClick={() => void handleBulkActivate()}
                  className="h-7 text-xs gap-1 cursor-pointer text-emerald-600 hover:text-emerald-700"
                >
                  <LuUserCheck className="size-3" />
                  <span>
                    {t("members.bulkActions.activate", {
                      defaultValue: "Mở khóa",
                    })}
                  </span>
                </LoadingButton>
              </div>
            </div>
          )}
          {bulkActionError ? (
            <p role="alert" className="text-xs text-destructive">
              {bulkActionError}
            </p>
          ) : null}

          {/* Role Permissions Summary */}
          <div className="rounded-lg border border-border/70 bg-muted/15 p-3 space-y-2">
            <div className="flex items-center gap-2 font-semibold text-foreground text-xs">
              <LuInfo className="size-3.5 text-primary" />
              <span>
                {t("members.rolePermissionsSummary", {
                  defaultValue: "Tóm tắt quyền hạn các vai trò:",
                })}
              </span>
            </div>
            <div className="grid grid-cols-4 gap-2 text-[11px] pt-1">
              <div className="rounded border border-border/50 bg-background/50 p-2 space-y-1">
                <div className="font-bold text-amber-500 flex items-center gap-1">
                  <LuShieldCheck className="size-3" />
                  <span>Owner</span>
                </div>
                <p className="text-muted-foreground text-[10px]">
                  Toàn quyền tối cao: Quản trị thanh toán, phân quyền và xóa
                  workspace.
                </p>
              </div>

              <div className="rounded border border-border/50 bg-background/50 p-2 space-y-1">
                <div className="font-bold text-primary flex items-center gap-1">
                  <LuShield className="size-3" />
                  <span>Admin</span>
                </div>
                <p className="text-muted-foreground text-[10px]">
                  Quản trị hồ sơ, phân chia nhóm, xem proxy và mời thành viên
                  mới.
                </p>
              </div>

              <div className="rounded border border-border/50 bg-background/50 p-2 space-y-1">
                <div className="font-bold text-emerald-500 flex items-center gap-1">
                  <LuPlay className="size-3" />
                  <span>Operator</span>
                </div>
                <p className="text-muted-foreground text-[10px]">
                  Chỉ chạy/mở profiles trong nhóm. Dữ liệu proxy và cookie bị
                  che.
                </p>
              </div>

              <div className="rounded border border-border/50 bg-background/50 p-2 space-y-1">
                <div className="font-bold text-muted-foreground flex items-center gap-1">
                  <LuUser className="size-3" />
                  <span>Member</span>
                </div>
                <p className="text-muted-foreground text-[10px]">
                  Chỉ nhìn thấy hồ sơ thuộc nhóm được Owner/Admin phân bổ cụ
                  thể.
                </p>
              </div>
            </div>
          </div>
        </div>

        <DialogFooter className="border-t border-border/50 pt-3 flex items-center justify-between sm:justify-between">
          <span className="text-xs text-muted-foreground">
            Tổng số:{" "}
            <strong className="text-foreground">{members.length}</strong> nhân
            sự
          </span>
          <RippleButton
            variant="outline"
            size="sm"
            onClick={onClose}
            className="h-8 text-xs font-medium cursor-pointer"
          >
            {t("common.buttons.close", { defaultValue: "Đóng" })}
          </RippleButton>
        </DialogFooter>
      </MemberFrame>

      {/* SUB-MODAL: Invite Member */}
      <Dialog open={inviteModalOpen} onOpenChange={setInviteModalOpen}>
        <DialogContent className="max-w-md p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold flex items-center gap-2">
              <LuUserPlus className="size-4 text-primary" />
              <span>
                {createInviteAccount
                  ? "Tạo tài khoản trong workspace"
                  : t("members.inviteModalTitle", {
                      defaultValue: "Mời Thành Viên Vào Không Gian Làm Việc",
                    })}
              </span>
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              {createInviteAccount
                ? "Tạo identity BugLogin và gán member/role vào workspace hiện tại."
                : t("members.inviteModalDesc", {
                    defaultValue:
                      "Tạo liên kết hoặc mã mời để gửi trực tiếp cho thành viên tham gia.",
                  })}
              <span className="mt-1 block">
                {createInviteAccount
                  ? "Identity được tạo ở trạng thái chờ xác minh email; không cấp mật khẩu và chưa thể đăng nhập. Role được kiểm tra lại ở API."
                  : "Chỉ người có account đang hoạt động và email đã xác minh mới dùng được mã mời. Luồng này không tạo account hoặc mật khẩu."}
              </span>
            </DialogDescription>
          </DialogHeader>

          {!generatedInvite ? (
            <form
              onSubmit={(e) => void handleCreateInvite(e)}
              className="space-y-4 py-2 text-xs"
            >
              {createInviteAccount && (
                <div className="space-y-1.5">
                  <Label className="text-xs font-medium">Tên tài khoản</Label>
                  <Input
                    value={inviteUsername}
                    onChange={(e) => setInviteUsername(e.target.value)}
                    minLength={3}
                    maxLength={64}
                    required
                    placeholder="nguyen.van.a"
                    className="h-9 text-xs"
                  />
                </div>
              )}
              <div className="space-y-1.5">
                <Label className="text-xs font-medium">
                  {createInviteAccount
                    ? "Email tài khoản"
                    : "Email nhận lời mời"}
                </Label>
                <Input
                  type="email"
                  placeholder="staff01@alphamedia.vn"
                  value={inviteEmail}
                  onChange={(e) => setInviteEmail(e.target.value)}
                  required
                  className="h-9 text-xs"
                />
              </div>
              {inviteError && (
                <p role="alert" className="text-xs text-destructive">
                  {inviteError}
                </p>
              )}

              <div className="space-y-1.5">
                <Label className="text-xs font-medium">
                  Chọn vai trò thực thi (Role)
                </Label>
                <Select
                  value={inviteRole}
                  onValueChange={(v) => setInviteRole(v as MemberRole)}
                >
                  <SelectTrigger className="h-9 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {assignableRoles.map((role) => (
                      <SelectItem key={role} value={role} className="text-xs">
                        {role === "admin"
                          ? "Admin — quản trị workspace"
                          : role === "lead"
                            ? "Lead — quản lý member và team cấp thấp hơn"
                            : role === "operator"
                              ? "Operator — tạo/cập nhật và chạy tài nguyên được cấp"
                              : "Member — đọc và chạy tài nguyên được cấp"}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              <DialogFooter className="border-t border-border/50 pt-3">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setInviteModalOpen(false)}
                  className="h-8 text-xs cursor-pointer"
                >
                  {t("common.buttons.cancel", { defaultValue: "Hủy" })}
                </Button>
                <LoadingButton
                  type="submit"
                  size="sm"
                  isLoading={isInviting}
                  disabled={
                    !inviteEmail.trim() ||
                    (createInviteAccount && inviteUsername.trim().length < 3)
                  }
                  className="h-8 text-xs font-medium cursor-pointer"
                >
                  {createInviteAccount
                    ? "Tạo account & gán role"
                    : t("members.inviteButton", {
                        defaultValue: "Sinh Mã Mời",
                      })}
                </LoadingButton>
              </DialogFooter>
            </form>
          ) : (
            <div className="space-y-4 py-2 text-xs">
              <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 p-3 space-y-1 text-emerald-600 dark:text-emerald-400">
                <div className="font-semibold flex items-center gap-1.5">
                  <LuCheck className="size-4" />
                  <span>Đã tạo lời mời thành công!</span>
                </div>
                <p className="text-[11px] opacity-90">
                  Gửi mã hoặc liên kết cho người nhận. Họ cần đăng nhập bằng tài
                  khoản có email đã xác minh rồi chấp nhận lời mời.
                </p>
              </div>

              {/* Invite Code Box */}
              <div className="space-y-1.5">
                <Label className="text-xs font-medium">
                  Mã mời trực tiếp (Invite Code):
                </Label>
                <div className="flex items-center gap-2">
                  <Input
                    readOnly
                    value={generatedInvite.code}
                    className="h-9 font-mono font-bold tracking-widest text-primary text-sm bg-muted/40"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => void handleCopyCode()}
                    className="h-9 px-3 gap-1.5 cursor-pointer"
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

              {/* Deep Link Box */}
              <div className="space-y-1.5">
                <Label className="text-xs font-medium">
                  Liên kết kích hoạt (Deep Link):
                </Label>
                <div className="flex items-center gap-2">
                  <Input
                    readOnly
                    value={generatedInvite.deepLink}
                    className="h-9 font-mono text-[11px] text-muted-foreground bg-muted/40"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => void handleCopyLink()}
                    className="h-9 px-3 gap-1.5 cursor-pointer"
                  >
                    {copiedLink ? (
                      <LuCheck className="size-3.5 text-emerald-500" />
                    ) : (
                      <LuLink className="size-3.5" />
                    )}
                    <span>{copiedLink ? "Đã chép" : "Copy"}</span>
                  </Button>
                </div>
              </div>

              <div className="text-[11px] text-muted-foreground">
                Thời hạn lời mời: Còn hiệu lực đến{" "}
                <strong className="text-foreground">
                  {generatedInvite.expiresAt}
                </strong>
                .
              </div>

              <DialogFooter className="border-t border-border/50 pt-3">
                <Button
                  size="sm"
                  onClick={() => setInviteModalOpen(false)}
                  className="h-8 text-xs font-medium cursor-pointer"
                >
                  Hoàn Tất
                </Button>
              </DialogFooter>
            </div>
          )}
        </DialogContent>
      </Dialog>

      {/* SUB-MODAL: Bulk Member Invitations */}
      <Dialog open={bulkModalOpen} onOpenChange={setBulkModalOpen}>
        <DialogContent className="max-w-xl p-6">
          <DialogHeader className="border-b border-border/50 pb-3">
            <DialogTitle className="text-base font-semibold flex items-center gap-2">
              <LuUsers className="size-4 text-primary" />
              <span>
                {bulkCreateAccounts
                  ? "Tạo tài khoản hàng loạt"
                  : t("members.bulkModal.title", {
                      defaultValue: "Mời thành viên hàng loạt",
                    })}
              </span>
            </DialogTitle>
            <DialogDescription className="text-xs text-muted-foreground">
              {bulkCreateAccounts
                ? "Tạo tối đa 100 identity và gán role vào workspace hiện tại."
                : t("members.bulkModal.desc", {
                    defaultValue:
                      "Nhập danh sách email để tạo lời mời dùng một lần.",
                  })}
              <span className="mt-1 block">
                {bulkCreateAccounts
                  ? "Dòng theo định dạng username,email. Identity chờ xác minh email và chưa thể đăng nhập; API kiểm tra quyền role cho từng account."
                  : "Người nhận phải có account BugLogin đang hoạt động với email đã xác minh; thao tác này không tạo account hay mật khẩu."}
              </span>
            </DialogDescription>
          </DialogHeader>

          {!bulkResults ? (
            <div className="space-y-4 py-2 text-xs">
              <div className="space-y-1.5">
                <Label className="text-xs font-medium">
                  {bulkCreateAccounts
                    ? "Danh sách username,email (mỗi dòng một tài khoản):"
                    : t("members.bulkModal.emailsLabel", {
                        defaultValue:
                          "Danh sách email (mỗi dòng một email hoặc phân cách bằng dấu phẩy):",
                      })}
                </Label>
                <textarea
                  rows={5}
                  value={bulkEmailsText}
                  onChange={(e) => {
                    setBulkEmailsText(e.target.value);
                    setBulkInputError(null);
                  }}
                  placeholder={
                    bulkCreateAccounts
                      ? "username01,staff01@example.com\nusername02,staff02@example.com"
                      : t("members.bulkModal.emailsPlaceholder", {
                          defaultValue:
                            "staff01@alphamedia.vn\nstaff02@alphamedia.vn\nstaff03@alphamedia.vn",
                        })
                  }
                  className="w-full rounded-md border border-input bg-transparent px-3 py-2 text-xs shadow-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring font-mono"
                  disabled={isBulkProcessing}
                />
                <span className="text-[11px] text-muted-foreground">
                  {bulkCreateAccounts
                    ? `${bulkAccountAnalysis.rows.length} hợp lệ · ${bulkAccountAnalysis.invalidLines.length} không hợp lệ · ${bulkAccountAnalysis.duplicateLines.length} trùng · tối đa 100`
                    : `${bulkEmailAnalysis.emails.length} hợp lệ · ${bulkEmailAnalysis.invalid.length} không hợp lệ · ${bulkEmailAnalysis.duplicates.length} trùng · tối đa 100`}
                  {bulkInputError && (
                    <span role="alert" className="block text-destructive">
                      {bulkInputError}
                    </span>
                  )}
                </span>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1.5">
                  <Label className="text-xs font-medium">
                    {t("members.bulkModal.roleLabel", {
                      defaultValue: "Vai trò gán cho nhân sự:",
                    })}
                  </Label>
                  <Select
                    value={bulkRole}
                    onValueChange={(v) => setBulkRole(v as MemberRole)}
                  >
                    <SelectTrigger className="h-9 text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {assignableRoles.map((r) => (
                        <SelectItem
                          key={r}
                          value={r}
                          className="text-xs capitalize"
                        >
                          {t(`roles.${r}`, { defaultValue: r })}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>

                <div className="space-y-1.5">
                  <Label className="text-xs font-medium">Phương thức</Label>
                  <p className="h-9 flex items-center text-muted-foreground">
                    {bulkCreateAccounts
                      ? "Tạo identity + gán member role"
                      : "Gửi lời mời dùng một lần"}
                  </p>
                </div>
              </div>

              {isBulkProcessing && (
                <div className="space-y-1.5 pt-2">
                  <div className="flex justify-between text-xs text-muted-foreground">
                    <span>
                      {t("members.bulkModal.processing", {
                        current: bulkProgress.current,
                        total: bulkProgress.total,
                        defaultValue: `Đang xử lý ${bulkProgress.current} / ${bulkProgress.total}...`,
                      })}
                    </span>
                    <span>
                      {Math.round(
                        (bulkProgress.current /
                          Math.max(1, bulkProgress.total)) *
                          100,
                      )}
                      %
                    </span>
                  </div>
                  <div className="h-2 w-full rounded-full bg-muted overflow-hidden">
                    <div
                      className="h-full bg-primary transition-all duration-200"
                      style={{
                        width: `${Math.round((bulkProgress.current / Math.max(1, bulkProgress.total)) * 100)}%`,
                      }}
                    />
                  </div>
                </div>
              )}

              <DialogFooter className="border-t border-border/50 pt-3">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setBulkModalOpen(false)}
                  disabled={isBulkProcessing}
                  className="h-8 text-xs cursor-pointer"
                >
                  {t("common.buttons.cancel", { defaultValue: "Hủy" })}
                </Button>
                <LoadingButton
                  type="button"
                  size="sm"
                  isLoading={isBulkProcessing}
                  onClick={() => void handleCreateBulkInvitations()}
                  disabled={
                    (bulkCreateAccounts
                      ? bulkAccountAnalysis.entryCount
                      : bulkEmailAnalysis.entryCount) === 0
                  }
                  className="h-8 text-xs font-medium cursor-pointer"
                >
                  {t("members.bulkModal.startButton", {
                    count: bulkCreateAccounts
                      ? bulkAccountAnalysis.rows.length
                      : bulkEmailAnalysis.emails.length,
                    defaultValue: bulkCreateAccounts
                      ? `Tạo ${bulkAccountAnalysis.rows.length} tài khoản`
                      : `Tạo ${bulkEmailAnalysis.emails.length} lời mời`,
                  })}
                </LoadingButton>
              </DialogFooter>
            </div>
          ) : (
            <div className="space-y-4 py-2 text-xs">
              <div
                role="status"
                className={cn(
                  "rounded-lg border p-3 space-y-1",
                  bulkFailureCount === 0
                    ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
                    : bulkSuccessCount > 0
                      ? "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300"
                      : "border-destructive/30 bg-destructive/10 text-destructive",
                )}
              >
                <div className="font-semibold flex items-center gap-1.5">
                  {bulkFailureCount === 0 ? (
                    <LuCheck className="size-4" />
                  ) : (
                    <LuInfo className="size-4" />
                  )}
                  <span>
                    {bulkFailureCount === 0
                      ? bulkCreateAccounts
                        ? "Đã tạo account và gán workspace"
                        : "Đã tạo đủ lời mời"
                      : bulkSuccessCount > 0
                        ? "Đã xử lý một phần"
                        : bulkCreateAccounts
                          ? "Không tạo được account"
                          : "Không tạo được lời mời"}
                  </span>
                </div>
                <p className="text-[11px] opacity-90">
                  {bulkSuccessCount} thành công · {bulkFailureCount} thất bại.
                  {bulkCreateAccounts
                    ? "Identity đang chờ xác minh email nên chưa thể đăng nhập; kiểm tra lỗi trước khi thử lại."
                    : "Mã mời chỉ hiển thị một lần; kiểm tra lỗi trước khi gửi lại."}
                </p>
              </div>

              <div className="max-h-60 overflow-y-auto rounded-lg border border-border">
                <table className="w-full text-left text-xs">
                  <thead className="bg-muted/50 border-b border-border sticky top-0 text-[11px] font-medium text-muted-foreground">
                    <tr>
                      <th className="py-2 px-3">Email</th>
                      <th className="py-2 px-3">
                        {bulkCreateAccounts
                          ? "Kết quả cấp quyền"
                          : "Mã mời / Liên kết"}
                      </th>
                      <th className="py-2 px-3 w-20 text-center">Trạng thái</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border/60">
                    {bulkResults.map((res) => (
                      <tr
                        key={res.email.toLocaleLowerCase("en-US")}
                        className="hover:bg-muted/20"
                      >
                        <td className="py-1.5 px-3 font-mono text-[11px]">
                          {res.email}
                        </td>
                        <td className="py-1.5 px-3 font-mono text-[11px]">
                          {res.status === "success" ? (
                            <span className="text-muted-foreground">
                              {bulkCreateAccounts
                                ? `${res.username} · ${bulkRole} · chờ xác minh`
                                : res.inviteCode}
                            </span>
                          ) : (
                            <span className="text-destructive text-[10px]">
                              {res.error}
                            </span>
                          )}
                        </td>
                        <td className="py-1.5 px-3 text-center">
                          {res.status === "success" ? (
                            <Badge className="bg-emerald-500/10 text-emerald-600 border-emerald-500/30 text-[10px]">
                              Thành công
                            </Badge>
                          ) : (
                            <Badge
                              variant="destructive"
                              className="text-[10px]"
                            >
                              Lỗi
                            </Badge>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              <DialogFooter className="border-t border-border/50 pt-3 flex items-center justify-between sm:justify-between">
                <div className="flex items-center gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => void handleCopyBulkResults()}
                    className="h-8 gap-1.5 text-xs cursor-pointer"
                  >
                    {bulkCopiedAll ? (
                      <LuCheck className="size-3.5 text-emerald-500" />
                    ) : (
                      <LuCopy className="size-3.5" />
                    )}
                    <span>
                      {bulkCopiedAll
                        ? t("members.bulkModal.copied", {
                            defaultValue: "Đã chép",
                          })
                        : t("members.bulkModal.copyCredentials", {
                            defaultValue: bulkCreateAccounts
                              ? "Sao chép danh sách account"
                              : "Sao chép mã mời",
                          })}
                    </span>
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={handleExportBulkCsv}
                    className="h-8 gap-1.5 text-xs cursor-pointer"
                  >
                    <LuExternalLink className="size-3.5" />
                    <span>
                      {t("members.bulkModal.exportCsv", {
                        defaultValue: "Xuất CSV",
                      })}
                    </span>
                  </Button>
                </div>
                <Button
                  size="sm"
                  onClick={() => {
                    setBulkResults(null);
                    setBulkEmailsText("");
                    setBulkModalOpen(false);
                  }}
                  className="h-8 text-xs font-medium cursor-pointer"
                >
                  {t("members.bulkModal.done", { defaultValue: "Hoàn tất" })}
                </Button>
              </DialogFooter>
            </div>
          )}
        </DialogContent>
      </Dialog>

      {/* SUB-MODAL 4: Offboarding & Revocation Modal (Section 13.5) */}
      {offboardMember && (
        <Dialog open={true} onOpenChange={() => setOffboardMember(null)}>
          <DialogContent className="max-w-md p-6">
            <DialogHeader className="border-b border-border/50 pb-3">
              <DialogTitle className="text-base font-semibold flex items-center gap-2 text-destructive">
                <LuShieldAlert className="size-4" />
                <span>
                  Tạm ngưng thành viên:{" "}
                  {offboardMember.identity?.username ?? "thành viên"}
                </span>
              </DialogTitle>
              <DialogDescription className="text-xs text-muted-foreground">
                API sẽ chuyển trạng thái thành viên trong workspace sang
                Suspended.
              </DialogDescription>
            </DialogHeader>

            <div className="py-2 text-xs text-muted-foreground">
              Chỉ trạng thái membership trong workspace thay đổi. Hành động này
              không chuyển quyền sở hữu profile.
            </div>

            <DialogFooter className="border-t border-border/50 pt-3">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setOffboardMember(null)}
                className="h-8 text-xs cursor-pointer"
              >
                Hủy Bỏ
              </Button>
              <LoadingButton
                type="button"
                variant="destructive"
                size="sm"
                isLoading={isOffboarding}
                onClick={() => void handleExecuteOffboarding()}
                className="h-8 text-xs font-medium cursor-pointer"
              >
                Tạm ngưng thành viên
              </LoadingButton>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    </>
  );
}
