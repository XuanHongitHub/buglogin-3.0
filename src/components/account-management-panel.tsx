"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuPlus, LuRefreshCw, LuSearch, LuUsers } from "react-icons/lu";
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
import { showSuccessToast } from "@/lib/toast-utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import {
  type AccountAdminRecord,
  type AccountCreatePendingInput,
  type AccountStatus,
} from "../../frontend/src/generated/command-registry.ts";

const desktopApi = createTauriDesktopApi();

type CreateMode = "single" | "bulk";
type CreateLine = {
  line: number;
  username: string;
  email: string;
  input: AccountCreatePendingInput;
  idempotencyKey: string;
};
type CreateResult = {
  line: number;
  username: string;
  email: string;
  error?: string;
};

function parseBulkRows(
  value: string,
): Array<{ line: number; username: string; email: string }> {
  return value.split(/\r?\n/).flatMap((raw, index) => {
    const text = raw.trim();
    if (!text || text.startsWith("#") || /^username\s*,\s*email$/i.test(text))
      return [];
    const separator = text.indexOf(",");
    if (separator < 1) return [{ line: index + 1, username: "", email: "" }];
    return [
      {
        line: index + 1,
        username: text.slice(0, separator).trim(),
        email: text.slice(separator + 1).trim(),
      },
    ];
  });
}

function createLineInput(
  username: string,
  email: string,
  jobId: string,
): CreateLine {
  const id = crypto.randomUUID().replaceAll("-", "");
  return {
    line: 1,
    username,
    email,
    input: { accountId: `acct_${id}`, username, email },
    idempotencyKey: `acct-create-${jobId}-${id}`,
  };
}

export function AccountManagementPanel({
  onCreateScoped,
}: {
  onCreateScoped: () => void;
}) {
  const { t } = useTranslation();
  const [records, setRecords] = useState<AccountAdminRecord[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState("all");
  const [showCreatePage, setShowCreatePage] = useState(false);
  const [mode, setMode] = useState<CreateMode>("single");
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [bulkText, setBulkText] = useState("");
  const [pendingLines, setPendingLines] = useState<CreateLine[]>([]);
  const [results, setResults] = useState<CreateResult[]>([]);
  const [isCreating, setIsCreating] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pendingStatus, setPendingStatus] = useState<{
    record: AccountAdminRecord;
    status: AccountStatus;
  } | null>(null);
  const [updatingAccountId, setUpdatingAccountId] = useState<string | null>(
    null,
  );
  const accountsRequestRef = useRef(0);
  const didLoadAccountsRef = useRef(false);

  const loadAccounts = useCallback(async () => {
    const request = ++accountsRequestRef.current;
    if (!didLoadAccountsRef.current) setIsLoading(true);
    else setIsRefreshing(true);
    try {
      const page = await desktopApi.listAccounts();
      if (request !== accountsRequestRef.current) return;
      setRecords(page.items);
      setLoadError(null);
    } catch (error) {
      if (request === accountsRequestRef.current)
        setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      if (request === accountsRequestRef.current) {
        didLoadAccountsRef.current = true;
        setIsLoading(false);
        setIsRefreshing(false);
      }
    }
  }, []);

  useEffect(() => {
    void loadAccounts();
  }, [loadAccounts]);

  useEffect(() => {
    const refreshWhenVisible = () => {
      if (document.visibilityState === "visible") void loadAccounts();
    };
    const timer = window.setInterval(() => void loadAccounts(), 30_000);
    window.addEventListener("focus", refreshWhenVisible);
    document.addEventListener("visibilitychange", refreshWhenVisible);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refreshWhenVisible);
      document.removeEventListener("visibilitychange", refreshWhenVisible);
      accountsRequestRef.current += 1;
    };
  }, [loadAccounts]);

  const visibleRecords = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return records.filter(({ account, identity }) => {
      if (statusFilter !== "all" && account.status !== statusFilter)
        return false;
      if (!query) return true;
      return [identity?.username, identity?.email, account.status]
        .filter(Boolean)
        .some((value) => value?.toLocaleLowerCase().includes(query));
    });
  }, [records, search, statusFilter]);

  const preparedLines = useMemo(() => {
    if (mode === "single") {
      if (!username.trim() && !email.trim()) return [];
      return [createLineInput(username.trim(), email.trim(), "single")];
    }
    const jobId = crypto.randomUUID().replaceAll("-", "");
    return parseBulkRows(bulkText).map((row) => ({
      ...createLineInput(row.username, row.email, jobId),
      line: row.line,
    }));
  }, [mode, username, email, bulkText]);

  const duplicateRows = useMemo(() => {
    const seenEmails = new Set<string>();
    const seenUsernames = new Set<string>();
    const duplicates = new Set<number>();
    for (const row of preparedLines) {
      const normalizedEmail = row.email.toLocaleLowerCase();
      const normalizedUsername = row.username.toLocaleLowerCase();
      if (
        seenEmails.has(normalizedEmail) ||
        seenUsernames.has(normalizedUsername)
      ) {
        duplicates.add(row.line);
      }
      seenEmails.add(normalizedEmail);
      seenUsernames.add(normalizedUsername);
    }
    return duplicates;
  }, [preparedLines]);

  const validateRows = (rows: CreateLine[]) => {
    const invalid = rows.filter(
      (row) =>
        row.username.length < 3 ||
        row.username.length > 64 ||
        !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(row.email) ||
        row.email.length > 254,
    );
    if (invalid.length > 0) {
      setActionError(
        t("accountAdmin.invalidRows", {
          defaultValue:
            "Kiểm tra username (3–64 ký tự) và email ở các dòng được đánh dấu.",
        }),
      );
      return false;
    }
    if (duplicateRows.size > 0) {
      setActionError(
        t("accountAdmin.duplicateRows", {
          defaultValue: "Danh sách có username hoặc email trùng nhau.",
        }),
      );
      return false;
    }
    if (rows.length > 100) {
      setActionError(
        t("accountAdmin.batchLimit", {
          defaultValue: "Mỗi lần chỉ tạo tối đa 100 account.",
        }),
      );
      return false;
    }
    if (rows.length === 0) {
      setActionError(
        t("accountAdmin.noRows", { defaultValue: "Nhập ít nhất một account." }),
      );
      return false;
    }
    return true;
  };

  const handleCreate = async () => {
    setActionError(null);
    setResults([]);
    if (!validateRows(preparedLines)) return;
    setPendingLines(preparedLines);
    setIsCreating(true);
    const created: CreateResult[] = [];
    const failed: CreateLine[] = [];
    try {
      for (const row of preparedLines) {
        try {
          await desktopApi.createPendingAccount(row.input, row.idempotencyKey);
          created.push({
            line: row.line,
            username: row.username,
            email: row.email,
          });
        } catch (error) {
          failed.push(row);
          created.push({
            line: row.line,
            username: row.username,
            email: row.email,
            error: error instanceof Error ? error.message : String(error),
          });
        }
        setResults([...created]);
      }
      setPendingLines(failed);
      await loadAccounts();
      if (failed.length === 0) {
        setUsername("");
        setEmail("");
        setBulkText("");
      }
    } finally {
      setIsCreating(false);
    }
  };

  const retryFailed = async () => {
    if (pendingLines.length === 0) return;
    setIsCreating(true);
    const nextResults = [...results];
    const remaining: CreateLine[] = [];
    try {
      for (const row of pendingLines) {
        try {
          await desktopApi.createPendingAccount(row.input, row.idempotencyKey);
          const index = nextResults.findIndex(
            (result) => result.line === row.line,
          );
          if (index >= 0)
            nextResults[index] = { ...nextResults[index], error: undefined };
        } catch (error) {
          remaining.push(row);
          const index = nextResults.findIndex(
            (result) => result.line === row.line,
          );
          if (index >= 0)
            nextResults[index] = {
              ...nextResults[index],
              error: error instanceof Error ? error.message : String(error),
            };
        }
      }
      setResults(nextResults);
      setPendingLines(remaining);
      await loadAccounts();
    } finally {
      setIsCreating(false);
    }
  };

  const updateStatus = async (
    record: AccountAdminRecord,
    status: AccountStatus,
  ) => {
    setUpdatingAccountId(record.account.id);
    setActionError(null);
    try {
      const account = await desktopApi.updateAccountStatus({
        accountId: record.account.id,
        status,
        expectedRevision: record.account.revision,
      });
      setRecords((current) =>
        current.map((item) =>
          item.account.id === account.id ? { ...item, account } : item,
        ),
      );
      setPendingStatus(null);
      showSuccessToast(
        t("accountAdmin.statusUpdated", {
          status,
          defaultValue: `Đã đổi trạng thái tài khoản thành ${status}.`,
        }),
      );
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setUpdatingAccountId(null);
    }
  };

  if (showCreatePage) {
    return (
      <section
        className="flex min-h-0 flex-1 flex-col overflow-auto p-5"
        aria-labelledby="account-create-title"
      >
        <div className="mx-auto w-full max-w-3xl">
          <div className="mb-5 flex items-start justify-between gap-4 border-b border-border pb-4">
            <div>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="mb-2 -ml-2"
                onClick={() => setShowCreatePage(false)}
                disabled={isCreating}
              >
                ← Tài khoản
              </Button>
              <h2 id="account-create-title" className="text-lg font-semibold">
                {t("accountAdmin.createTitle", { defaultValue: "Tạo account" })}
              </h2>
              <p className="mt-1 text-sm text-muted-foreground">
                Tạo một hoặc nhiều account ở trạng thái chờ xác minh.
              </p>
            </div>
            <Badge variant="outline">Chờ xác minh</Badge>
          </div>
          <div className="space-y-5">
            <div
              className="flex gap-2 border-b pb-3"
              role="group"
              aria-label="Kiểu tạo account"
            >
              {(["single", "bulk"] as const).map((value) => (
                <Button
                  key={value}
                  type="button"
                  aria-pressed={mode === value}
                  variant={mode === value ? "default" : "outline"}
                  size="sm"
                  onClick={() => {
                    setMode(value);
                    setResults([]);
                    setActionError(null);
                  }}
                >
                  {value === "single" ? "Một account" : "Hàng loạt"}
                </Button>
              ))}
            </div>
            {mode === "single" ? (
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1.5">
                  <Label htmlFor="new-account-username">Username</Label>
                  <Input
                    id="new-account-username"
                    autoComplete="off"
                    value={username}
                    onChange={(event) => setUsername(event.target.value)}
                    minLength={3}
                    maxLength={64}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="new-account-email">Email</Label>
                  <Input
                    id="new-account-email"
                    type="email"
                    autoComplete="off"
                    value={email}
                    onChange={(event) => setEmail(event.target.value)}
                    maxLength={254}
                  />
                </div>
              </div>
            ) : (
              <div className="space-y-2">
                <Label htmlFor="bulk-account-rows">
                  Mỗi dòng: username,email
                </Label>
                <Textarea
                  id="bulk-account-rows"
                  value={bulkText}
                  onChange={(event) => setBulkText(event.target.value)}
                  placeholder={
                    "username,email\nnguyenan,an@example.com\nlebinh,binh@example.com"
                  }
                  rows={10}
                  spellCheck={false}
                />
                <p className="text-xs text-muted-foreground">
                  {preparedLines.length} / 100 dòng sẽ được gửi tuần tự; nếu có
                  lỗi, các dòng thành công vẫn được giữ.
                </p>
              </div>
            )}
            <p className="rounded-md border border-border bg-muted/30 p-3 text-sm text-muted-foreground">
              Tạo xong account chưa thể đăng nhập. Admin cần hoàn tất xác minh
              danh tính và cấp credentials qua luồng riêng.
            </p>
            {actionError ? (
              <p role="alert" className="text-sm text-destructive">
                {actionError}
              </p>
            ) : null}
            {results.length > 0 ? (
              <div
                className="max-h-64 overflow-auto rounded-md border border-border p-3 text-sm"
                aria-live="polite"
              >
                <h3 className="mb-2 font-medium">Kết quả tạo account</h3>
                {results.map((result, index) => (
                  <p
                    key={`${result.line}-${result.email}-${index}`}
                    className={
                      result.error ? "text-destructive" : "text-foreground"
                    }
                  >
                    Dòng {result.line}: {result.username} · {result.email} —{" "}
                    {result.error
                      ? `Lỗi: ${result.error}`
                      : "Đã tạo, chờ xác minh"}
                  </p>
                ))}
              </div>
            ) : null}
            <div className="flex flex-wrap justify-between gap-2 border-t border-border pt-4">
              <Button
                type="button"
                variant="outline"
                onClick={() => setShowCreatePage(false)}
                disabled={isCreating}
              >
                Quay lại danh sách
              </Button>
              <div className="flex flex-wrap gap-2">
                {pendingLines.length > 0 && results.length > 0 ? (
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => void retryFailed()}
                    disabled={isCreating}
                  >
                    Thử lại {pendingLines.length} dòng lỗi
                  </Button>
                ) : null}
                <Button
                  type="button"
                  onClick={() => void handleCreate()}
                  disabled={isCreating || preparedLines.length === 0}
                >
                  {isCreating
                    ? "Đang tạo…"
                    : mode === "single"
                      ? "Tạo account"
                      : `Tạo ${preparedLines.length} account`}
                </Button>
              </div>
            </div>
          </div>
        </div>
      </section>
    );
  }

  return (
    <section
      className="flex min-h-0 flex-1 flex-col p-5"
      aria-labelledby="account-admin-title"
    >
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 id="account-admin-title" className="text-base font-semibold">
            {t("accountAdmin.title", { defaultValue: "Tài khoản" })}
          </h2>
          <p className="mt-1 text-xs text-muted-foreground">
            {t("accountAdmin.description", {
              defaultValue:
                "Tạo một hoặc nhiều account ở trạng thái chờ xác minh và quản lý trạng thái hiện tại.",
            })}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => void loadAccounts()}
            disabled={isRefreshing}
          >
            <LuRefreshCw
              className={`mr-2 size-3.5 ${isRefreshing ? "animate-spin" : ""}`}
            />
            {t("common.refresh", { defaultValue: "Làm mới" })}
          </Button>
          <Button size="sm" onClick={onCreateScoped}>
            <LuPlus className="mr-2 size-3.5" />
            Tạo trong Org / Workspace
          </Button>
        </div>
      </div>

      <div className="mb-3 flex flex-wrap items-center gap-2">
        <div className="relative min-w-52 flex-1">
          <LuSearch className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Tìm username hoặc email"
            className="h-9 pl-8"
          />
        </div>
        <Select value={statusFilter} onValueChange={setStatusFilter}>
          <SelectTrigger className="h-9 w-44">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">Tất cả trạng thái</SelectItem>
            <SelectItem value="active">Đang hoạt động</SelectItem>
            <SelectItem value="suspended">Tạm khóa</SelectItem>
            <SelectItem value="disabled">Đã vô hiệu hóa</SelectItem>
          </SelectContent>
        </Select>
        <Badge
          variant="outline"
          className="h-9 gap-2 px-3 text-xs"
          aria-label={`${visibleRecords.length} kết quả trên ${records.length} tài khoản`}
        >
          <LuUsers className="size-3.5" /> {visibleRecords.length} /{" "}
          {records.length}
        </Badge>
      </div>

      <div className="min-h-0 flex-1 overflow-auto rounded-lg border border-border">
        {isLoading && records.length === 0 ? (
          <div
            className="p-8 text-center text-sm text-muted-foreground"
            role="status"
          >
            Đang tải danh sách tài khoản…
          </div>
        ) : loadError && records.length === 0 ? (
          <div className="p-8 text-center">
            <p className="text-sm text-destructive">
              Không tải được danh sách: {loadError}
            </p>
            <Button
              variant="outline"
              size="sm"
              className="mt-3"
              onClick={() => void loadAccounts()}
            >
              Thử lại
            </Button>
          </div>
        ) : visibleRecords.length === 0 ? (
          <div className="flex flex-col items-center gap-2 p-8 text-center">
            <p className="text-sm text-muted-foreground">
              {records.length === 0
                ? "Chưa có account."
                : "Không có kết quả phù hợp với bộ lọc hiện tại."}
            </p>
            {records.length > 0 ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  setSearch("");
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
                  Username / email
                </th>
                <th scope="col" className="px-3 py-2.5 font-medium">
                  Xác minh
                </th>
                <th scope="col" className="px-3 py-2.5 font-medium">
                  Trạng thái
                </th>
                <th scope="col" className="px-3 py-2.5 font-medium">
                  Tạo lúc
                </th>
                <th scope="col" className="px-3 py-2.5 font-medium">
                  Quản lý
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border/70">
              {visibleRecords.map((record) => {
                const { account, identity } = record;
                const canChange = account.status !== "disabled";
                return (
                  <tr key={account.id}>
                    <th scope="row" className="max-w-72 px-3 py-3 font-medium">
                      <div className="truncate">
                        {identity?.username ?? "Identity unavailable"}
                      </div>
                      <div className="truncate text-xs font-normal text-muted-foreground">
                        {identity?.email ?? "—"}
                      </div>
                    </th>
                    <td className="px-3 py-3">
                      {identity ? (
                        <Badge
                          variant={
                            identity.verification === "verified"
                              ? "default"
                              : "secondary"
                          }
                        >
                          {identity.verification === "verified"
                            ? "Đã xác minh"
                            : "Chờ xác minh"}
                        </Badge>
                      ) : (
                        <span className="text-muted-foreground">
                          Không có dữ liệu
                        </span>
                      )}
                    </td>
                    <td className="px-3 py-3">
                      <Badge
                        variant={
                          account.status === "active" ? "default" : "outline"
                        }
                      >
                        {account.status}
                      </Badge>
                    </td>
                    <td className="px-3 py-3 text-xs text-muted-foreground">
                      {new Date(account.createdAt).toLocaleString()}
                    </td>
                    <td className="px-3 py-3">
                      <Select
                        value={account.status}
                        disabled={!canChange || updatingAccountId !== null}
                        onValueChange={(value) =>
                          setPendingStatus({
                            record,
                            status: value as AccountStatus,
                          })
                        }
                      >
                        <SelectTrigger className="h-8 w-40">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {account.status === "active" ? (
                            <SelectItem value="suspended">Tạm khóa</SelectItem>
                          ) : null}
                          {account.status === "active" ||
                          account.status === "suspended" ? (
                            <SelectItem value="disabled">
                              Vô hiệu hóa
                            </SelectItem>
                          ) : null}
                          {account.status === "suspended" ? (
                            <SelectItem value="active">
                              Kích hoạt lại
                            </SelectItem>
                          ) : null}
                          {account.status === "disabled" ? (
                            <SelectItem value="disabled">
                              Đã vô hiệu hóa
                            </SelectItem>
                          ) : null}
                        </SelectContent>
                      </Select>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
      {pendingStatus ? (
        <div className="mt-2 flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-card px-3 py-2 text-xs">
          <span>
            Đổi trạng thái tài khoản{" "}
            <strong className="font-medium">
              {pendingStatus.record.identity?.email ??
                pendingStatus.record.identity?.username ??
                "đã chọn"}
            </strong>{" "}
            thành{" "}
            <strong className="font-medium">{pendingStatus.status}</strong>?
          </span>
          <div className="flex items-center gap-2">
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={updatingAccountId !== null}
              onClick={() => setPendingStatus(null)}
            >
              Hủy
            </Button>
            <Button
              type="button"
              size="sm"
              disabled={updatingAccountId !== null}
              onClick={() =>
                void updateStatus(pendingStatus.record, pendingStatus.status)
              }
            >
              {updatingAccountId === pendingStatus.record.account.id
                ? "Đang lưu…"
                : "Xác nhận"}
            </Button>
          </div>
        </div>
      ) : null}
      {loadError && records.length > 0 ? (
        <p role="status" className="mt-2 text-xs text-destructive">
          Làm mới thất bại; đang hiển thị dữ liệu cũ. {loadError}
        </p>
      ) : null}
      {actionError ? (
        <p role="alert" className="mt-2 text-xs text-destructive">
          {actionError}
        </p>
      ) : null}
    </section>
  );
}
