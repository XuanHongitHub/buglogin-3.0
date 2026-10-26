"use client";

import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuArrowLeft, LuUserPlus, LuUsers } from "react-icons/lu";
import { Button } from "@/components/ui/button";
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
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type { MemberRole } from "../../frontend/src/generated/command-registry";

const desktopApi = createTauriDesktopApi();
const ROLE_RANK: Record<MemberRole, number> = {
  owner: 4,
  admin: 3,
  lead: 2,
  operator: 1,
  member: 0,
};
const ROLES: MemberRole[] = ["owner", "admin", "lead", "operator", "member"];
const MIN_INITIAL_PASSWORD_LENGTH = 12;
const MAX_INITIAL_PASSWORD_LENGTH = 256;
const passwordLength = (value: string) => Array.from(value).length;

interface DirectAccountRow {
  line: number;
  username: string;
  email: string;
  initialPassword: string;
}

function analyzeDirectAccountRows(input: string) {
  const entries = input
    .split(/\r?\n/)
    .map((text, index) => ({ line: index + 1, text }))
    .filter(
      ({ text }) =>
        text.trim() &&
        !text.trimStart().startsWith("#") &&
        !/^username\s*,\s*email\s*,\s*initialpassword$/i.test(text.trim()),
    );
  const rows: DirectAccountRow[] = [];
  const invalidLines: number[] = [];
  const duplicateLines: number[] = [];
  const usernames = new Set<string>();
  const emails = new Set<string>();
  for (const { line, text } of entries) {
    const firstComma = text.indexOf(",");
    const secondComma = firstComma < 0 ? -1 : text.indexOf(",", firstComma + 1);
    if (firstComma < 0 || secondComma < 0) {
      invalidLines.push(line);
      continue;
    }
    const username = text.slice(0, firstComma).trim();
    const email = text.slice(firstComma + 1, secondComma).trim();
    const initialPassword = text.slice(secondComma + 1);
    if (
      username.length < 3 ||
      username.length > 64 ||
      email.length > 254 ||
      !/^[^\s@,;]+@[^\s@,;]+\.[^\s@,;]+$/.test(email) ||
      passwordLength(initialPassword) < MIN_INITIAL_PASSWORD_LENGTH ||
      passwordLength(initialPassword) > MAX_INITIAL_PASSWORD_LENGTH
    ) {
      invalidLines.push(line);
      continue;
    }
    const usernameKey = username.toLocaleLowerCase("en-US");
    const emailKey = email.toLocaleLowerCase("en-US");
    if (usernames.has(usernameKey) || emails.has(emailKey)) {
      duplicateLines.push(line);
      continue;
    }
    usernames.add(usernameKey);
    emails.add(emailKey);
    rows.push({ line, username, email, initialPassword });
  }
  return {
    rows,
    invalidLines,
    duplicateLines,
    exceedsLimit: entries.length > 100,
    entryCount: entries.length,
  };
}

function safeProvisioningErrorCode(error: unknown): string | null {
  if (!error || typeof error !== "object" || !("code" in error)) return null;
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" && /^[a-z0-9_]{1,64}$/.test(code)
    ? code
    : null;
}

interface ScopedAccountCreationPageProps {
  workspaceId: string;
  workspaceName: string;
  organizationName?: string;
  initialMode?: "single" | "bulk";
  onBack: () => void;
  onCreated?: () => void;
}

export function ScopedAccountCreationPage({
  workspaceId,
  workspaceName,
  organizationName,
  initialMode = "single",
  onBack,
  onCreated,
}: ScopedAccountCreationPageProps) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<"single" | "bulk">(initialMode);
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [initialPassword, setInitialPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [bulkRows, setBulkRows] = useState("");
  const [role, setRole] = useState<MemberRole>("member");
  const [assignableRoles, setAssignableRoles] = useState<MemberRole[]>([]);
  const [authorityError, setAuthorityError] = useState<string | null>(null);
  const [isLoadingAuthority, setIsLoadingAuthority] = useState(true);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  const bulkAnalysis = useMemo(
    () => analyzeDirectAccountRows(bulkRows),
    [bulkRows],
  );

  useEffect(() => {
    let disposed = false;
    async function loadAuthority() {
      setIsLoadingAuthority(true);
      setAuthorityError(null);
      try {
        const [account, members] = await Promise.all([
          desktopApi.getAccount(),
          desktopApi.listMembers(workspaceId),
        ]);
        const ownMembership = members.items.find(
          (member) =>
            member.accountId === account.accountId &&
            member.status === "active",
        );
        const isOrganizationOwner =
          account.managedWorkspaceIds.includes(workspaceId);
        const rank = account.platformAdmin
          ? Number.POSITIVE_INFINITY
          : isOrganizationOwner
            ? ROLE_RANK.admin + 1
            : ROLE_RANK[ownMembership?.role ?? "member"];
        const roles = ROLES.filter((candidate) => ROLE_RANK[candidate] < rank);
        if (!disposed) {
          setAssignableRoles(roles);
          setRole(roles[0] ?? "member");
        }
      } catch (error) {
        if (!disposed) {
          setAuthorityError(String(error));
          showErrorToast(
            `Không thể xác định quyền tạo tài khoản: ${String(error)}`,
          );
        }
      } finally {
        if (!disposed) setIsLoadingAuthority(false);
      }
    }
    void loadAuthority();
    return () => {
      disposed = true;
    };
  }, [workspaceId]);

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (isLoadingAuthority || authorityError || assignableRoles.length === 0)
      return;
    if (mode === "single") {
      if (
        passwordLength(initialPassword) < MIN_INITIAL_PASSWORD_LENGTH ||
        passwordLength(initialPassword) > MAX_INITIAL_PASSWORD_LENGTH
      ) {
        setInitialPassword("");
        setConfirmPassword("");
        setResult("Mật khẩu ban đầu phải dài từ 12 đến 256 ký tự.");
        return;
      }
      if (initialPassword !== confirmPassword) {
        setInitialPassword("");
        setConfirmPassword("");
        setResult("Mật khẩu xác nhận không khớp.");
        return;
      }
    }
    const submittedPassword = initialPassword;
    const submittedBulkRows = bulkRows;
    setIsSubmitting(true);
    setResult(null);
    if (mode === "bulk") setBulkRows("");
    try {
      if (mode === "single") {
        const accountId = `acct_${crypto.randomUUID().replaceAll("-", "")}`;
        const receipt = await desktopApi.createScopedPendingAccount(
          {
            workspaceId,
            accountId,
            username: username.trim(),
            email: email.trim(),
            role,
            initialPassword: submittedPassword,
          },
          `acct-create-${crypto.randomUUID()}`,
        );
        if (
          receipt.member?.status !== "active" ||
          receipt.loginEnabled !== true
        ) {
          throw new Error(
            "API chưa xác nhận tài khoản đã được tạo và membership đã active.",
          );
        }
        setResult(
          `${username.trim()} (${email.trim()}) đã được tạo và có thể đăng nhập ngay bằng mật khẩu ban đầu; tài khoản đã được gán vào ${workspaceName}.`,
        );
        setUsername("");
        setEmail("");
        setInitialPassword("");
        setConfirmPassword("");
        showSuccessToast("Đã tạo tài khoản trong workspace.");
        onCreated?.();
        return;
      }

      const submittedAnalysis = analyzeDirectAccountRows(submittedBulkRows);
      if (
        submittedAnalysis.exceedsLimit ||
        submittedAnalysis.invalidLines.length ||
        submittedAnalysis.duplicateLines.length ||
        submittedAnalysis.rows.length === 0
      ) {
        throw new Error(
          "Danh sách có dòng sai, trùng hoặc vượt quá 100 tài khoản. Mỗi dòng cần username,email,mật_khẩu_ban_đầu (12–256 ký tự). Hãy nhập lại danh sách rồi thử lại.",
        );
      }

      const runId = crypto.randomUUID();
      const created: string[] = [];
      const failed: Array<{ email: string; code: string | null }> = [];
      for (const row of submittedAnalysis.rows) {
        const accountId = `acct_${crypto.randomUUID().replaceAll("-", "")}`;
        try {
          const receipt = await desktopApi.createScopedPendingAccount(
            {
              workspaceId,
              accountId,
              username: row.username,
              email: row.email,
              role,
              initialPassword: row.initialPassword,
            },
            `acct-bulk-${runId}-${row.line}`,
          );
          if (
            receipt.member?.status !== "active" ||
            receipt.loginEnabled !== true
          ) {
            throw new Error("Tài khoản chưa sẵn sàng đăng nhập.");
          }
          created.push(row.email);
        } catch (error) {
          failed.push({
            email: row.email,
            code: safeProvisioningErrorCode(error),
          });
        }
      }
      const summary = `${created.length}/${submittedAnalysis.rows.length} tài khoản đã tạo và đăng nhập được${failed.length ? `; ${failed.length} dòng lỗi: ${failed.map(({ email, code }) => `${email}${code ? ` (${code})` : ""}`).join(" | ")}` : ""}.`;
      setResult(summary);
      if (created.length) {
        showSuccessToast(`Đã tạo ${created.length} tài khoản trong workspace.`);
        onCreated?.();
      }
      if (failed.length) showErrorToast(summary);
      if (failed.length) {
        setResult(
          `${created.length}/${submittedAnalysis.rows.length} tài khoản đã tạo và đăng nhập được. Không tạo được: ${failed.map(({ email, code }) => `${email}${code ? ` (${code})` : ""}`).join(", ")}. Mật khẩu đã được xóa; nhập lại mật khẩu cho các dòng lỗi trước khi thử lại.`,
        );
      }
    } catch (error) {
      const code = safeProvisioningErrorCode(error);
      const message =
        mode === "bulk"
          ? `Không thể xử lý danh sách tài khoản${code ? ` (${code})` : ""}. Mật khẩu đã được xóa; hãy kiểm tra dữ liệu và nhập lại.`
          : `Không thể tạo tài khoản${code ? ` (${code})` : ""}. Mật khẩu đã được xóa; hãy thử lại.`;
      setResult(message);
      showErrorToast(message);
    } finally {
      setIsSubmitting(false);
      setInitialPassword("");
      setConfirmPassword("");
    }
  };

  const blocked =
    isLoadingAuthority ||
    !!authorityError ||
    assignableRoles.length === 0 ||
    isSubmitting;

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto bg-background">
      <header className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
        <div className="flex items-center gap-3">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={onBack}
            aria-label="Quay lại workspace"
          >
            <LuArrowLeft className="size-4" />
          </Button>
          <div>
            <h1 className="text-sm font-semibold">Tạo tài khoản</h1>
            <p className="text-xs text-muted-foreground">
              {organizationName ? `${organizationName} / ` : ""}Workspace:{" "}
              {workspaceName}
            </p>
          </div>
        </div>
        <div
          role="tablist"
          aria-label="Chế độ tạo tài khoản"
          className="flex items-center rounded-md border border-border bg-muted/30 p-0.5"
        >
          <Button
            type="button"
            size="sm"
            variant={mode === "single" ? "secondary" : "ghost"}
            role="tab"
            aria-selected={mode === "single"}
            onClick={() => {
              setMode("single");
              setBulkRows("");
              setInitialPassword("");
              setConfirmPassword("");
            }}
          >
            <LuUserPlus className="mr-1.5 size-3.5" /> Tạo một
          </Button>
          <Button
            type="button"
            size="sm"
            variant={mode === "bulk" ? "secondary" : "ghost"}
            role="tab"
            aria-selected={mode === "bulk"}
            onClick={() => {
              setMode("bulk");
              setInitialPassword("");
              setConfirmPassword("");
            }}
          >
            <LuUsers className="mr-1.5 size-3.5" /> Tạo hàng loạt
          </Button>
        </div>
      </header>

      <div className="mx-auto w-full max-w-2xl space-y-5 px-5 py-5">
        <div className="space-y-1">
          <h2 className="text-sm font-semibold">
            Tài khoản được cấp quyền đăng nhập ngay
          </h2>
          <p className="text-xs text-muted-foreground">
            Đây là tài khoản mới, không phải lời mời. Chuyển mật khẩu ban đầu
            cho người dùng qua kênh an toàn.
          </p>
        </div>

        {authorityError ? (
          <section
            role="alert"
            className="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
          >
            Không thể tải quyền trong workspace. {authorityError}
          </section>
        ) : assignableRoles.length === 0 && !isLoadingAuthority ? (
          <section
            role="status"
            className="rounded-md border border-border p-3 text-sm text-muted-foreground"
          >
            Tài khoản hiện tại không có vai trò thấp hơn để gán trong workspace
            này.
          </section>
        ) : null}

        <form
          onSubmit={(event) => void handleSubmit(event)}
          className="space-y-5"
        >
          {mode === "single" && (
            <section className="space-y-3">
              <div className="border-b border-border/60 pb-2">
                <h3 className="text-xs font-semibold">Thông tin tài khoản</h3>
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1.5">
                  <Label htmlFor="account-username">Tên tài khoản</Label>
                  <Input
                    id="account-username"
                    autoComplete="off"
                    minLength={3}
                    maxLength={64}
                    required
                    value={username}
                    onChange={(event) => setUsername(event.target.value)}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="account-email">Email</Label>
                  <Input
                    id="account-email"
                    type="email"
                    autoComplete="off"
                    maxLength={254}
                    required
                    value={email}
                    onChange={(event) => setEmail(event.target.value)}
                  />
                </div>
              </div>
            </section>
          )}

          <section className="space-y-3 border-t border-border/60 pt-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="space-y-1">
                <h3 className="text-xs font-semibold">Quyền trong workspace</h3>
                <p className="text-xs text-muted-foreground">
                  Chỉ hiển thị vai trò bạn có quyền cấp.
                </p>
              </div>
              <Select
                value={assignableRoles.includes(role) ? role : undefined}
                onValueChange={(value) => setRole(value as MemberRole)}
                disabled={blocked}
              >
                <SelectTrigger id="account-role" className="w-48">
                  <SelectValue
                    placeholder={
                      isLoadingAuthority ? "Đang tải vai trò…" : "Chọn vai trò"
                    }
                  />
                </SelectTrigger>
                <SelectContent>
                  {assignableRoles.map((candidate) => (
                    <SelectItem key={candidate} value={candidate}>
                      {t(`roles.${candidate}`, { defaultValue: candidate })}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </section>

          {mode === "single" ? (
            <section className="space-y-3 border-t border-border/60 pt-4">
              <div className="border-b border-border/60 pb-2">
                <h3 className="text-xs font-semibold">Thông tin đăng nhập</h3>
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1.5">
                  <Label htmlFor="account-initial-password">
                    Mật khẩu ban đầu
                  </Label>
                  <Input
                    id="account-initial-password"
                    type="password"
                    autoComplete="new-password"
                    minLength={MIN_INITIAL_PASSWORD_LENGTH}
                    maxLength={MAX_INITIAL_PASSWORD_LENGTH}
                    required
                    value={initialPassword}
                    onChange={(event) => setInitialPassword(event.target.value)}
                  />
                  <p className="text-xs text-muted-foreground">
                    Dài 12–256 ký tự. Hãy chuyển mật khẩu cho người dùng qua
                    kênh an toàn.
                  </p>
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="account-confirm-password">
                    Xác nhận mật khẩu
                  </Label>
                  <Input
                    id="account-confirm-password"
                    type="password"
                    autoComplete="new-password"
                    minLength={MIN_INITIAL_PASSWORD_LENGTH}
                    maxLength={MAX_INITIAL_PASSWORD_LENGTH}
                    required
                    value={confirmPassword}
                    onChange={(event) => setConfirmPassword(event.target.value)}
                  />
                </div>
              </div>
            </section>
          ) : (
            <section className="space-y-2 border-t border-border/60 pt-4">
              <Label htmlFor="bulk-accounts">Danh sách tài khoản</Label>
              <p className="text-xs text-muted-foreground">
                Mỗi dòng theo định dạng{" "}
                <code>username,email,mật_khẩu_ban_đầu</code>. Mật khẩu riêng
                từng tài khoản, dài 12–256 ký tự. Tối đa 100 dòng.
              </p>
              <textarea
                id="bulk-accounts"
                value={bulkRows}
                onChange={(event) => setBulkRows(event.target.value)}
                rows={10}
                spellCheck={false}
                className="flex w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
                placeholder="username,email,initial-password"
              />
              <p className="text-xs text-muted-foreground">
                {bulkAnalysis.rows.length} hợp lệ ·{" "}
                {bulkAnalysis.invalidLines.length} dòng sai ·{" "}
                {bulkAnalysis.duplicateLines.length} dòng trùng
              </p>
            </section>
          )}

          {result && (
            <p
              role="status"
              className="rounded-md border border-border bg-muted/30 p-3 text-sm"
            >
              {result}
            </p>
          )}
          <div className="flex justify-end gap-2 border-t border-border/60 pt-3">
            <Button type="button" variant="outline" onClick={onBack}>
              Quay lại
            </Button>
            <Button
              type="submit"
              disabled={
                blocked ||
                (mode === "bulk" && bulkAnalysis.rows.length === 0) ||
                (mode === "single" &&
                  (passwordLength(initialPassword) <
                    MIN_INITIAL_PASSWORD_LENGTH ||
                    passwordLength(initialPassword) >
                      MAX_INITIAL_PASSWORD_LENGTH ||
                    initialPassword !== confirmPassword))
              }
            >
              {isSubmitting
                ? "Đang tạo…"
                : mode === "single"
                  ? "Tạo tài khoản"
                  : `Tạo ${bulkAnalysis.rows.length} tài khoản`}
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
