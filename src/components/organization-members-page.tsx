"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  LuArrowLeft,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuUserCheck,
  LuUserMinus,
  LuUsers,
} from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
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
import { Textarea } from "@/components/ui/textarea";
import {
  analyzeOrganizationAccountRows,
  validateOrganizationAccountFields,
} from "../../apps/desktop/src/organization-account-input";
import type {
  AccountProvisioningReceipt,
  OrganizationAccountCreateInput,
  OrganizationMember,
  OrganizationMemberPage,
  OrganizationMemberStatus,
  OrganizationMemberStatusUpdateInput,
  OrganizationMemberView,
} from "../../frontend/src/generated/command-registry";

type MemberIdentity = { username: string; email: string };
type OrganizationMemberRow = OrganizationMemberView & {
  identity?: MemberIdentity;
};

export interface OrganizationMembersPageProps {
  organizationId: string;
  organizationName: string;
  canInviteMember?: boolean;
  onBack: () => void;
  onInviteMember: () => void;
  listMembers: (organizationId: string) => Promise<OrganizationMemberPage>;
  createAccount: (
    input: OrganizationAccountCreateInput,
    idempotencyKey: string,
  ) => Promise<AccountProvisioningReceipt>;
  updateMemberStatus: (
    input: OrganizationMemberStatusUpdateInput,
  ) => Promise<OrganizationMember>;
}

type CreateMode = "single" | "bulk";
type CreateResult = {
  line: number;
  username: string;
  email: string;
  errorCode?: string;
};

function safeErrorCode(error: unknown): string | undefined {
  if (!error || typeof error !== "object" || !("code" in error))
    return undefined;
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" && /^[a-z0-9_]{1,64}$/.test(code)
    ? code
    : undefined;
}

function isForbiddenError(error: unknown): boolean {
  if (!error || typeof error !== "object") return false;
  const problem = error as { status?: unknown; code?: unknown };
  return problem.status === 403 || problem.code === "forbidden";
}

export function OrganizationMembersPage({
  organizationId,
  organizationName,
  canInviteMember = true,
  onBack,
  onInviteMember,
  listMembers,
  createAccount,
  updateMemberStatus,
}: OrganizationMembersPageProps) {
  const [members, setMembers] = useState<OrganizationMemberRow[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [forbidden, setForbidden] = useState(false);
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState("all");
  const [createPageOpen, setCreatePageOpen] = useState(false);
  const [mode, setMode] = useState<CreateMode>("single");
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [bulkText, setBulkText] = useState("");
  const [results, setResults] = useState<CreateResult[]>([]);
  const [createError, setCreateError] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);
  const [updatingAccountId, setUpdatingAccountId] = useState<string | null>(
    null,
  );
  const [actionError, setActionError] = useState<string | null>(null);

  const loadMembers = useCallback(
    async (background = false) => {
      setForbidden(false);
      if (background) setIsRefreshing(true);
      else setIsLoading(true);
      try {
        const page = await listMembers(organizationId);
        setMembers(
          page.items.map((member) => ({
            ...member,
            identity:
              (member.username ?? "").trim() || (member.email ?? "").trim()
                ? {
                    username: member.username ?? "",
                    email: member.email ?? "",
                  }
                : undefined,
          })),
        );
        setLoadError(null);
      } catch (error) {
        setForbidden(isForbiddenError(error));
        setLoadError(error instanceof Error ? error.message : String(error));
      } finally {
        setIsLoading(false);
        setIsRefreshing(false);
      }
    },
    [listMembers, organizationId],
  );

  useEffect(() => {
    void loadMembers();
  }, [loadMembers]);

  const bulk = useMemo(
    () => analyzeOrganizationAccountRows(bulkText),
    [bulkText],
  );
  const singleError = validateOrganizationAccountFields({
    username,
    email,
    password,
    confirmPassword,
  });
  const singleValid = singleError === null;
  const visibleMembers = members.filter((member) => {
    if (statusFilter !== "all" && member.status !== statusFilter) return false;
    const search = query.trim().toLocaleLowerCase();
    if (!search) return true;
    return [member.identity?.username, member.identity?.email].some((value) =>
      value?.toLocaleLowerCase().includes(search),
    );
  });
  const missingIdentityCount = visibleMembers.filter(
    (member) => !member.identity,
  ).length;

  const submitCreate = async (event: React.FormEvent) => {
    event.preventDefault();
    setCreateError(null);
    setResults([]);
    const rows =
      mode === "single"
        ? singleValid
          ? [
              {
                line: 1,
                username: username.trim(),
                email: email.trim(),
                password,
              },
            ]
          : []
        : bulk.rows;
    if (mode === "single" && !singleValid) {
      setCreateError(
        singleError === "invalid_username"
          ? "Username phải dài từ 3 đến 64 ký tự."
          : singleError === "invalid_email"
            ? "Nhập email hợp lệ, tối đa 254 ký tự."
            : singleError === "invalid_password_length"
              ? "Mật khẩu ban đầu phải dài từ 12 đến 256 ký tự."
              : "Mật khẩu xác nhận không khớp.",
      );
      return;
    }
    if (
      mode === "bulk" &&
      (!rows.length ||
        bulk.invalidLines.length ||
        bulk.duplicateLines.length ||
        bulk.exceedsLimit)
    ) {
      setCreateError(
        "Danh sách phải có tối đa 100 dòng hợp lệ, không trùng username/email; mỗi dòng là username,email,mật_khẩu_ban_đầu.",
      );
      return;
    }
    const submitted = rows.map((row) => ({ ...row }));
    setIsCreating(true);
    setPassword("");
    setConfirmPassword("");
    setBulkText("");
    const nextResults: CreateResult[] = [];
    try {
      for (const row of submitted) {
        const accountId = `acct_${crypto.randomUUID().replaceAll("-", "")}`;
        try {
          const receipt = await createAccount(
            {
              organizationId,
              accountId,
              username: row.username,
              email: row.email,
              initialPassword: row.password,
            },
            `org-account-create-${crypto.randomUUID()}`,
          );
          if (
            receipt.loginEnabled !== true ||
            receipt.organizationMember?.status !== "active"
          ) {
            throw new Error(
              "API chưa xác nhận account đã đăng nhập được và active trong tổ chức.",
            );
          }
          nextResults.push({
            line: row.line,
            username: row.username,
            email: row.email,
          });
        } catch (error) {
          nextResults.push({
            line: row.line,
            username: row.username,
            email: row.email,
            errorCode: safeErrorCode(error) ?? "create_failed",
          });
        }
        setResults([...nextResults]);
      }
      const succeeded = nextResults.filter(
        (result) => !result.errorCode,
      ).length;
      if (succeeded) await loadMembers(true);
      if (nextResults.every((result) => !result.errorCode)) {
        setUsername("");
        setEmail("");
      }
    } finally {
      setIsCreating(false);
    }
  };

  const changeStatus = async (member: OrganizationMemberRow) => {
    if (!member.identity || updatingAccountId) return;
    setUpdatingAccountId(member.accountId);
    setActionError(null);
    const status: OrganizationMemberStatus =
      member.status === "active" ? "suspended" : "active";
    try {
      const updated = await updateMemberStatus({
        organizationId,
        accountId: member.accountId,
        status,
        expectedRevision: member.revision,
      });
      setMembers((current) =>
        current.map((item) =>
          item.accountId === updated.accountId
            ? { ...item, ...updated, identity: member.identity }
            : item,
        ),
      );
    } catch (error) {
      setActionError(
        isForbiddenError(error)
          ? "API từ chối thao tác này (403). Tài khoản hiện tại không có quyền CAP-ORG-001 phù hợp."
          : error instanceof Error
            ? error.message
            : String(error),
      );
    } finally {
      setUpdatingAccountId(null);
    }
  };

  if (forbidden) {
    return (
      <main className="flex min-h-0 flex-1 flex-col bg-background">
        <header className="flex items-center gap-3 border-b border-border/60 px-4 py-3 md:px-6">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={onBack}
            aria-label="Quay lại tổ chức"
          >
            <LuArrowLeft className="size-4" />
          </Button>
          <h1 className="text-base font-semibold">Không có quyền truy cập</h1>
        </header>
        <section className="m-auto max-w-xl p-6 text-center">
          <p role="alert" className="text-sm font-medium">
            API từ chối yêu cầu quản lý thành viên tổ chức (403).
          </p>
          <p className="mt-2 text-sm text-muted-foreground">
            Tài khoản có thể mở trang theo vai trò hiện có, nhưng cần được cấp
            quyền CAP-ORG-001 phù hợp cho tổ chức này.
          </p>
          <Button
            type="button"
            variant="outline"
            className="mt-4"
            onClick={onBack}
          >
            Quay lại tổ chức
          </Button>
        </section>
      </main>
    );
  }

  if (createPageOpen) {
    return (
      <main
        className="flex min-h-0 flex-1 flex-col overflow-y-auto bg-background"
        aria-labelledby="org-account-create-title"
      >
        <header className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-4 py-3 md:px-6">
          <div className="flex items-center gap-3">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              onClick={() => setCreatePageOpen(false)}
              disabled={isCreating}
              aria-label="Quay lại thành viên tổ chức"
            >
              <LuArrowLeft className="size-4" />
            </Button>
            <div>
              <h1
                id="org-account-create-title"
                className="text-base font-semibold"
              >
                Tạo tài khoản tổ chức
              </h1>
              <p className="text-xs text-muted-foreground">
                {organizationName} · tài khoản mới được thêm vào tổ chức, chưa
                gán workspace
              </p>
            </div>
          </div>
          <div
            className="flex items-center gap-1 rounded-md border border-border p-1"
            role="group"
            aria-label="Cách tạo tài khoản"
          >
            <Button
              type="button"
              size="sm"
              variant={mode === "single" ? "secondary" : "ghost"}
              aria-pressed={mode === "single"}
              onClick={() => {
                setMode("single");
                setBulkText("");
                setResults([]);
                setCreateError(null);
              }}
              disabled={isCreating}
            >
              Một tài khoản
            </Button>
            <Button
              type="button"
              size="sm"
              variant={mode === "bulk" ? "secondary" : "ghost"}
              aria-pressed={mode === "bulk"}
              onClick={() => {
                setMode("bulk");
                setPassword("");
                setConfirmPassword("");
                setResults([]);
                setCreateError(null);
              }}
              disabled={isCreating}
            >
              Hàng loạt
            </Button>
          </div>
        </header>
        <div className="mx-auto w-full max-w-3xl space-y-5 p-4 md:p-6">
          <p className="border-b border-border pb-3 text-sm text-muted-foreground">
            Đây là tạo account trực tiếp, không phải lời mời. Người dùng đặt mật
            khẩu ban đầu ở đây và có thể đăng nhập sau khi API xác nhận tạo
            thành công.
          </p>
          <form
            onSubmit={(event) => void submitCreate(event)}
            className="space-y-4"
          >
            {mode === "single" ? (
              <div className="grid gap-4 sm:grid-cols-2">
                <div className="space-y-1.5">
                  <Label htmlFor="org-account-username">Tên tài khoản</Label>
                  <Input
                    id="org-account-username"
                    autoComplete="off"
                    minLength={3}
                    maxLength={64}
                    required
                    value={username}
                    onChange={(event) => setUsername(event.target.value)}
                    disabled={isCreating}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="org-account-email">Email</Label>
                  <Input
                    id="org-account-email"
                    type="email"
                    autoComplete="off"
                    maxLength={254}
                    required
                    value={email}
                    onChange={(event) => setEmail(event.target.value)}
                    disabled={isCreating}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="org-account-password">Mật khẩu ban đầu</Label>
                  <Input
                    id="org-account-password"
                    type="password"
                    autoComplete="new-password"
                    minLength={12}
                    maxLength={256}
                    required
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    disabled={isCreating}
                  />
                  <p className="text-xs text-muted-foreground">
                    Dài 12–256 ký tự. Chuyển mật khẩu qua kênh an toàn.
                  </p>
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="org-account-confirm-password">
                    Xác nhận mật khẩu
                  </Label>
                  <Input
                    id="org-account-confirm-password"
                    type="password"
                    autoComplete="new-password"
                    minLength={12}
                    maxLength={256}
                    required
                    value={confirmPassword}
                    onChange={(event) => setConfirmPassword(event.target.value)}
                    disabled={isCreating}
                  />
                </div>
              </div>
            ) : (
              <div className="space-y-2">
                <Label htmlFor="org-account-bulk">Danh sách tài khoản</Label>
                <p className="text-xs text-muted-foreground">
                  Mỗi dòng: username,email,mật_khẩu_ban_đầu (4–256 ký tự), tối
                  đa 100 dòng.
                </p>
                <Textarea
                  id="org-account-bulk"
                  rows={10}
                  spellCheck={false}
                  autoComplete="off"
                  value={bulkText}
                  onChange={(event) => setBulkText(event.target.value)}
                  disabled={isCreating}
                  placeholder={
                    "username,email,initial-password\nnguyenan,an@example.com,1234"
                  }
                />
                <p className="text-xs text-muted-foreground">
                  {bulk.rows.length} hợp lệ · {bulk.invalidLines.length} dòng
                  sai · {bulk.duplicateLines.length} dòng trùng
                  {bulk.exceedsLimit ? " · vượt giới hạn 100 dòng" : ""}
                </p>
              </div>
            )}
            {createError ? (
              <p role="alert" className="text-sm text-destructive">
                {createError}
              </p>
            ) : null}
            {results.length ? (
              <section
                aria-live="polite"
                className="max-h-64 overflow-auto border-y border-border py-3 text-sm"
              >
                <h2 className="mb-2 font-medium">Kết quả tạo tài khoản</h2>
                <ul className="space-y-1">
                  {results.map((result) => (
                    <li
                      key={`${result.line}-${result.email}`}
                      className={
                        result.errorCode
                          ? "text-destructive"
                          : "text-foreground"
                      }
                    >
                      Dòng {result.line}: {result.username} · {result.email} —{" "}
                      {result.errorCode
                        ? result.errorCode === "forbidden" ||
                          result.errorCode.endsWith("_forbidden")
                          ? "API từ chối (403): cần quyền CAP-ORG-001 phù hợp."
                          : `Không tạo được (${result.errorCode})`
                        : "Đã tạo, đăng nhập được"}
                    </li>
                  ))}
                </ul>
              </section>
            ) : null}
            <div className="flex flex-wrap justify-between gap-2 border-t border-border pt-4">
              <Button
                type="button"
                variant="outline"
                onClick={() => setCreatePageOpen(false)}
                disabled={isCreating}
              >
                Quay lại danh sách
              </Button>
              <Button
                type="submit"
                disabled={
                  isCreating ||
                  (mode === "single"
                    ? !singleValid
                    : !bulk.rows.length ||
                      !!bulk.invalidLines.length ||
                      !!bulk.duplicateLines.length ||
                      bulk.exceedsLimit)
                }
              >
                {isCreating
                  ? "Đang tạo…"
                  : mode === "single"
                    ? "Tạo tài khoản"
                    : `Tạo ${bulk.rows.length} tài khoản`}
              </Button>
            </div>
          </form>
        </div>
      </main>
    );
  }

  return (
    <main
      className="flex min-h-0 flex-1 flex-col overflow-hidden bg-background"
      aria-labelledby="org-members-title"
    >
      <header className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-4 py-3 md:px-6">
        <div className="flex min-w-0 items-center gap-3">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={onBack}
            aria-label="Quay lại tổ chức"
          >
            <LuArrowLeft className="size-4" />
          </Button>
          <div className="min-w-0">
            <p className="truncate text-xs text-muted-foreground">
              Tổ chức / {organizationName}
            </p>
            <h1 id="org-members-title" className="text-base font-semibold">
              Thành viên tổ chức
            </h1>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={onInviteMember}
            disabled={!canInviteMember}
            title={
              canInviteMember
                ? undefined
                : "Cần có workspace bạn có thể quản lý để mời thành viên."
            }
          >
            <LuUserCheck className="mr-2 size-3.5" />
            Mời vào workspace
          </Button>
          <Button
            type="button"
            size="sm"
            onClick={() => {
              setCreatePageOpen(true);
              setResults([]);
              setCreateError(null);
            }}
          >
            <LuPlus className="mr-2 size-3.5" />
            Tạo tài khoản
          </Button>
        </div>
      </header>
      <section className="flex min-h-0 flex-1 flex-col gap-3 p-4 md:p-6">
        <div className="flex flex-wrap items-center gap-2">
          <div className="relative min-w-52 flex-1">
            <LuSearch className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              aria-label="Tìm thành viên theo username hoặc email"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Tìm username hoặc email"
              className="h-9 pl-8"
            />
          </div>
          <Select value={statusFilter} onValueChange={setStatusFilter}>
            <SelectTrigger
              className="h-9 w-44"
              aria-label="Lọc trạng thái thành viên"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">Tất cả trạng thái</SelectItem>
              <SelectItem value="active">Đang hoạt động</SelectItem>
              <SelectItem value="suspended">Tạm ngưng</SelectItem>
            </SelectContent>
          </Select>
          <Badge variant="outline" className="h-9 gap-2 px-3 text-xs">
            <LuUsers className="size-3.5" />
            {visibleMembers.length} / {members.length}
          </Badge>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void loadMembers(true)}
            disabled={isRefreshing}
          >
            <LuRefreshCw
              className={`mr-2 size-3.5 ${isRefreshing ? "animate-spin" : ""}`}
            />
            Làm mới
          </Button>
        </div>
        {missingIdentityCount > 0 ? (
          <p role="status" className="text-xs text-muted-foreground">
            API chưa cung cấp username/email cho {missingIdentityCount} thành
            viên đang hiển thị. Các thao tác theo từng tài khoản bị khóa để
            tránh tác động nhầm.
          </p>
        ) : null}
        {actionError ? (
          <p role="alert" className="text-sm text-destructive">
            Không cập nhật được trạng thái thành viên. {actionError}
          </p>
        ) : null}
        {loadError && members.length > 0 ? (
          <p role="status" className="text-xs text-destructive">
            Làm mới thất bại; đang hiển thị dữ liệu cũ. {loadError}
          </p>
        ) : null}
        <div className="min-h-0 flex-1 overflow-auto rounded-md border border-border">
          {isLoading && members.length === 0 ? (
            <p
              role="status"
              className="p-8 text-center text-sm text-muted-foreground"
            >
              Đang tải thành viên tổ chức…
            </p>
          ) : loadError && members.length === 0 ? (
            <div className="p-8 text-center">
              <p role="alert" className="text-sm text-destructive">
                Không tải được thành viên: {loadError}
              </p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="mt-3"
                onClick={() => void loadMembers()}
              >
                Thử lại
              </Button>
            </div>
          ) : visibleMembers.length === 0 ? (
            <div className="p-8 text-center">
              <p className="text-sm text-muted-foreground">
                {members.length
                  ? "Không có thành viên phù hợp với bộ lọc."
                  : "Tổ chức chưa có thành viên."}
              </p>
              {members.length ? (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="mt-3"
                  onClick={() => {
                    setQuery("");
                    setStatusFilter("all");
                  }}
                >
                  Xóa bộ lọc
                </Button>
              ) : null}
            </div>
          ) : (
            <table className="w-full border-collapse text-left text-sm">
              <thead className="sticky top-0 z-10 border-b bg-muted/70 text-xs text-muted-foreground">
                <tr>
                  <th scope="col" className="px-3 py-2.5 font-medium">
                    Tài khoản
                  </th>
                  <th scope="col" className="px-3 py-2.5 font-medium">
                    Trạng thái
                  </th>
                  <th
                    scope="col"
                    className="px-3 py-2.5 text-right font-medium"
                  >
                    Quản lý
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border/70">
                {visibleMembers.map((member) => (
                  <tr key={member.accountId}>
                    <th scope="row" className="px-3 py-3 font-medium">
                      {member.identity ? (
                        <>
                          <div className="truncate">
                            {member.identity.username}
                          </div>
                          <div className="truncate text-xs font-normal text-muted-foreground">
                            {member.identity.email}
                          </div>
                        </>
                      ) : (
                        <span className="text-sm text-muted-foreground">
                          Thông tin định danh chưa được cấp bởi API
                        </span>
                      )}
                    </th>
                    <td className="px-3 py-3">
                      <Badge
                        variant={
                          member.status === "active" ? "default" : "outline"
                        }
                      >
                        {member.status === "active"
                          ? "Đang hoạt động"
                          : "Tạm ngưng"}
                      </Badge>
                    </td>
                    <td className="px-3 py-3 text-right">
                      {member.identity ? (
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          disabled={updatingAccountId !== null}
                          onClick={() => void changeStatus(member)}
                        >
                          {updatingAccountId === member.accountId ? (
                            "Đang lưu…"
                          ) : member.status === "active" ? (
                            <>
                              <LuUserMinus className="mr-1.5 size-3.5" />
                              Tạm ngưng
                            </>
                          ) : (
                            <>
                              <LuUserCheck className="mr-1.5 size-3.5" />
                              Kích hoạt lại
                            </>
                          )}
                        </Button>
                      ) : (
                        <span className="text-xs text-muted-foreground">
                          Định danh bắt buộc
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </section>
    </main>
  );
}
