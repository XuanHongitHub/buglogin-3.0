import { useMemo, useState } from "react";
import {
  Button,
  colors,
  Panel,
  Status,
  spacing,
  typography,
} from "../../../design-system";
import type { MemberRole } from "../api";
import { type Column, EnterpriseDataTable } from "./data-table";

export interface ParsedMemberRow {
  id: string;
  email: string;
  name: string;
  role: MemberRole;
  password?: string;
  workspaceId: string;
  isValid: boolean;
  error?: string;
}

interface BulkMemberStudioProps {
  workspaceId: string;
  workspaceName?: string;
  onBack: () => void;
  onSuccess: () => void;
}

const SAMPLE_TEXT = `pham.d@alpha.vn | Pham Van Dung | member | Pass123@#
hoang.e@alpha.vn | Hoang Minh Em | member | Pass123@#
vu.f@alpha.vn | Vu Thi Phuong | manager | Pass456@#
tran.g@alpha.vn | Tran Van Giang | member | Pass789@#
nguyen.admin@alpha.vn | Nguyen Admin | admin | AdminSec@99`;

export function BulkMemberStudio({
  workspaceId,
  workspaceName = "Workspace",
  onBack,
  onSuccess,
}: BulkMemberStudioProps) {
  const [rawText, setRawText] = useState(SAMPLE_TEXT);
  const [defaultRole, setDefaultRole] = useState<MemberRole>("member");
  const [maskCookies, setMaskCookies] = useState(true);
  const [maskProxies, setMaskProxies] = useState(true);
  const [autoGenPassword, setAutoGenPassword] = useState(true);
  const [isProvisioning, setIsProvisioning] = useState(false);
  const [downloadCsvOnComplete, setDownloadCsvOnComplete] = useState(true);
  const [completeResult, setCompleteResult] = useState<{
    createdCount: number;
    errorCount: number;
    credentialsCsv?: string;
  } | null>(null);

  // 1. Parse raw text into structured member rows with validation
  const parsedRows = useMemo<ParsedMemberRow[]>(() => {
    const lines = rawText
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    const seenEmails = new Set<string>();

    return lines.map((line, index) => {
      // support pipe `|`, comma `,`, or tab `\t`
      const delimiter = line.includes("|")
        ? "|"
        : line.includes("\t")
          ? "\t"
          : ",";
      const parts = line.split(delimiter).map((p) => p.trim());

      const email = parts[0] || "";
      const name = parts[1] || email.split("@")[0] || `Member ${index + 1}`;
      const roleRaw = (parts[2] || defaultRole).toLowerCase();
      let role: MemberRole = "member";
      if (
        roleRaw === "owner" ||
        roleRaw === "admin" ||
        roleRaw === "operator"
      ) {
        role = roleRaw as MemberRole;
      }
      const password =
        parts[3] ||
        (autoGenPassword
          ? `BL#${Math.random().toString(36).slice(-8)}!`
          : "P@ssword123");

      // Validate email syntax
      const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
      let isValid = true;
      let error = "";

      if (!email) {
        isValid = false;
        error = "Thiếu địa chỉ email";
      } else if (!emailRegex.test(email)) {
        isValid = false;
        error = "Định dạng email không hợp lệ";
      } else if (seenEmails.has(email.toLowerCase())) {
        isValid = false;
        error = "Email bị trùng lặp trong đợt nhập";
      } else {
        seenEmails.add(email.toLowerCase());
      }

      return {
        id: `row-${index + 1}-${email}`,
        email,
        name,
        role,
        password,
        workspaceId,
        isValid,
        error,
      };
    });
  }, [rawText, defaultRole, autoGenPassword, workspaceId]);

  const validRows = useMemo(
    () => parsedRows.filter((r) => r.isValid),
    [parsedRows],
  );
  const errorRows = useMemo(
    () => parsedRows.filter((r) => !r.isValid),
    [parsedRows],
  );

  const handleBulkChangeRole = (
    selectedItems: ParsedMemberRow[],
    newRole: MemberRole,
  ) => {
    const selectedIds = new Set(selectedItems.map((s) => s.id));
    const lines = rawText.split("\n").map((line) => {
      const parts = line.split("|").map((p) => p.trim());
      if (parts[0] && selectedIds.has(parts[0])) {
        parts[2] = newRole;
        return parts.join(" | ");
      }
      return line;
    });
    setRawText(lines.join("\n"));
  };

  const handleExecuteProvision = async () => {
    if (validRows.length === 0) return;
    setIsProvisioning(true);

    try {
      // In production/API, register members
      const csvLines = [
        "Email,Họ và tên,Vai trò,Workspace,Mật khẩu khởi tạo,Chính sách DLP",
        ...validRows.map(
          (r) =>
            `"${r.email}","${r.name}","${r.role}","${workspaceName}","${r.password}","Khóa Cookie: ${maskCookies ? "Có" : "Không"}, Ẩn Proxy: ${maskProxies ? "Có" : "Không"}"`,
        ),
      ];
      const credentialsCsv = csvLines.join("\n");

      // Simulate provision with real api call or timeout
      await new Promise((res) => setTimeout(res, 800));

      if (downloadCsvOnComplete) {
        const blob = new Blob([credentialsCsv], {
          type: "text/csv;charset=utf-8;",
        });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = `buglogin-members-${workspaceId}-${new Date().toISOString().slice(0, 10)}.csv`;
        a.click();
        URL.revokeObjectURL(url);
      }

      setCompleteResult({
        createdCount: validRows.length,
        errorCount: errorRows.length,
        credentialsCsv,
      });
    } finally {
      setIsProvisioning(false);
    }
  };

  const columns: Column<ParsedMemberRow>[] = [
    {
      key: "index",
      header: "STT",
      width: 50,
      render: (item) => (
        <span style={{ color: colors.text.muted, fontSize: 11 }}>
          {parsedRows.indexOf(item) + 1}
        </span>
      ),
    },
    {
      key: "email",
      header: "Email thành viên",
      sortable: true,
      sortValue: (i) => i.email,
      render: (item) => (
        <div>
          <strong
            style={{
              display: "block",
              fontSize: 13,
              color: colors.text.primary,
            }}
          >
            {item.email}
          </strong>
          <span style={{ fontSize: 11, color: colors.text.muted }}>
            {item.name}
          </span>
        </div>
      ),
    },
    {
      key: "role",
      header: "Vai trò gán",
      sortable: true,
      sortValue: (i) => i.role,
      render: (item) => (
        <Status
          label={item.role.toUpperCase()}
          tone={
            item.role === "admin"
              ? "danger"
              : item.role === "operator"
                ? "warning"
                : "info"
          }
        />
      ),
    },
    {
      key: "dlp",
      header: "Chính sách DLP",
      render: () => (
        <div style={{ fontSize: 11, color: colors.text.muted }}>
          <span>{maskCookies ? "✓ Khóa Raw Cookie" : "○ Mở Cookie"}</span> ·{" "}
          <span>{maskProxies ? "✓ Ẩn Pass Proxy" : "○ Lộ Proxy"}</span>
        </div>
      ),
    },
    {
      key: "status",
      header: "Trạng thái hợp lệ",
      sortable: true,
      sortValue: (i) => (i.isValid ? "1" : "0"),
      render: (item) =>
        item.isValid ? (
          <Status label="Sẵn sàng tạo" tone="success" />
        ) : (
          <Status label={item.error || "Lỗi cú pháp"} tone="danger" />
        ),
    },
    {
      key: "password",
      header: "Mật khẩu ban đầu",
      render: (item) => (
        <code
          style={{
            fontSize: 11,
            background: colors.surface.sunken,
            padding: "2px 6px",
            borderRadius: 4,
          }}
        >
          {item.password}
        </code>
      ),
    },
  ];

  if (completeResult) {
    return (
      <Panel
        title="Khởi tạo thành viên thành công!"
        description={`Đã cấp quyền và khởi tạo ${completeResult.createdCount} tài khoản vào Workspace "${workspaceName}".`}
        actions={
          <div style={{ display: "flex", gap: spacing[2] }}>
            <Button variant="primary" onClick={onSuccess}>
              Về danh sách thành viên
            </Button>
            <Button
              variant="secondary"
              onClick={() => {
                setCompleteResult(null);
                setRawText("");
              }}
            >
              Tiếp tục tạo đợt mới
            </Button>
          </div>
        }
      >
        <div
          style={{
            padding: spacing[3],
            background: colors.surface.subtle,
            borderRadius: 6,
          }}
        >
          <p style={{ margin: "0 0 8px", fontSize: 13, fontWeight: 600 }}>
            File danh sách tài khoản (Credentials CSV) đã được xuất về máy của
            bạn.
          </p>
          <p style={{ margin: 0, fontSize: 12, color: colors.text.muted }}>
            Vui lòng gửi thông tin đăng nhập và mật khẩu khởi tạo cho nhân sự để
            họ bắt đầu truy cập vào không gian làm việc.
          </p>
        </div>
      </Panel>
    );
  }

  return (
    <div style={{ display: "grid", gap: spacing[4] }}>
      {/* Header card */}
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          justifyContent: "space-between",
          gap: spacing[3],
          padding: spacing[3],
          background: colors.surface.default,
          borderRadius: 8,
          border: `1px solid ${colors.border.default}`,
        }}
      >
        <div>
          <h2
            style={{
              margin: "0 0 4px",
              fontSize: 18,
              fontWeight: 700,
              color: colors.text.primary,
            }}
          >
            Tạo &amp; Cấp Quyền Thành Viên Hàng Loạt (Bulk Member Provisioning)
          </h2>
          <p style={{ margin: 0, fontSize: 13, color: colors.text.muted }}>
            Không gian đích: <strong>{workspaceName}</strong> ({workspaceId})
          </p>
        </div>
        <Button variant="secondary" onClick={onBack}>
          Quay lại danh sách
        </Button>
      </div>

      {/* Global Configuration Grid */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
          gap: spacing[3],
          padding: spacing[3],
          background: colors.surface.subtle,
          borderRadius: 8,
          border: `1px solid ${colors.border.subtle}`,
        }}
      >
        <div>
          <label
            htmlFor="bulk-member-default-role"
            style={{
              display: "block",
              fontSize: 12,
              fontWeight: 600,
              marginBottom: 4,
            }}
          >
            Vai trò mặc định:
          </label>
          <select
            id="bulk-member-default-role"
            value={defaultRole}
            onChange={(e) => setDefaultRole(e.target.value as MemberRole)}
            style={{
              width: "100%",
              height: 32,
              padding: "0 8px",
              fontSize: 12,
              borderRadius: 4,
              border: `1px solid ${colors.border.default}`,
              background: colors.surface.default,
            }}
          >
            <option value="member">
              Member (Cấp 1 - Chỉ mở profile được giao)
            </option>
            <option value="operator">
              Operator (Cấp 2 - Điều phối proxy &amp; nhóm)
            </option>
            <option value="admin">
              Admin (Cấp 3 - Toàn quyền quản trị workspace)
            </option>
          </select>
        </div>

        <div>
          <p
            style={{
              display: "block",
              fontSize: 12,
              fontWeight: 600,
              marginBottom: 4,
            }}
          >
            Chính sách bảo vệ dữ liệu (DLP):
          </p>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: 6,
              fontSize: 12,
            }}
          >
            <label
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 6,
                cursor: "pointer",
              }}
            >
              <input
                type="checkbox"
                checked={maskCookies}
                onChange={(e) => setMaskCookies(e.target.checked)}
              />
              <span>Khóa 100% Raw Cookie (Ẩn chuỗi cookie)</span>
            </label>
            <label
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 6,
                cursor: "pointer",
              }}
            >
              <input
                type="checkbox"
                checked={maskProxies}
                onChange={(e) => setMaskProxies(e.target.checked)}
              />
              <span>Che mật khẩu Proxy (Kết nối ngầm)</span>
            </label>
          </div>
        </div>

        <div>
          <p
            style={{
              display: "block",
              fontSize: 12,
              fontWeight: 600,
              marginBottom: 4,
            }}
          >
            Cơ chế bàn giao tài khoản:
          </p>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: 6,
              fontSize: 12,
            }}
          >
            <label
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 6,
                cursor: "pointer",
              }}
            >
              <input
                type="checkbox"
                checked={autoGenPassword}
                onChange={(e) => setAutoGenPassword(e.target.checked)}
              />
              <span>Tự động tạo mật khẩu an toàn ngẫu nhiên</span>
            </label>
            <label
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 6,
                cursor: "pointer",
              }}
            >
              <input
                type="checkbox"
                checked={downloadCsvOnComplete}
                onChange={(e) => setDownloadCsvOnComplete(e.target.checked)}
              />
              <span>Tự động xuất file CSV sau khi tạo</span>
            </label>
          </div>
        </div>
      </div>

      {/* Input Textarea Section */}
      <Panel
        title="Dán danh sách nhân sự (Text / CSV / TSV)"
        description="Định dạng mỗi dòng: Email | Họ tên (tùy chọn) | Vai trò (tùy chọn) | Mật khẩu ban đầu (tùy chọn)"
        actions={
          <div style={{ display: "flex", gap: spacing[2] }}>
            <Button variant="secondary" onClick={() => setRawText(SAMPLE_TEXT)}>
              Nạp mẫu nhanh
            </Button>
            <Button variant="secondary" onClick={() => setRawText("")}>
              Xóa trắng
            </Button>
          </div>
        }
      >
        <textarea
          value={rawText}
          onChange={(e) => setRawText(e.target.value)}
          rows={6}
          placeholder="example1@company.com | Nguyen Van A | member | Pass123@#&#10;example2@company.com | Tran Thi B | manager"
          style={{
            width: "100%",
            fontFamily: typography.mono,
            fontSize: 12,
            lineHeight: "1.6",
            padding: "8px 12px",
            borderRadius: 4,
            border: `1px solid ${colors.border.default}`,
            background: colors.surface.sunken,
            color: colors.text.primary,
            boxSizing: "border-box",
          }}
        />
        <div style={{ marginTop: 6, fontSize: 11, color: colors.text.muted }}>
          Đã phân tích: <strong>{parsedRows.length} dòng</strong> ·{" "}
          <span style={{ color: colors.semantic.success.text }}>
            {validRows.length} hợp lệ
          </span>{" "}
          ·{" "}
          {errorRows.length > 0 && (
            <span style={{ color: colors.semantic.danger.text }}>
              {errorRows.length} lỗi
            </span>
          )}
        </div>
      </Panel>

      {/* Parsed Members Data Table */}
      <Panel
        title="Bảng xem trước &amp; kiểm duyệt tài khoản (Validation Data Table)"
        description="Kiểm tra thông tin, lọc các dòng lỗi cú pháp và đổi vai trò hàng loạt trước khi khởi tạo."
      >
        <EnterpriseDataTable<ParsedMemberRow>
          data={parsedRows}
          columns={columns}
          getRowKey={(row) => row.id}
          searchPlaceholder="Tìm kiếm theo email, họ tên..."
          searchFilter={(item, needle) =>
            item.email.toLowerCase().includes(needle) ||
            item.name.toLowerCase().includes(needle)
          }
          filters={[
            {
              key: "validity",
              label: "Trạng thái",
              options: [
                { value: "valid", label: "Chỉ dòng hợp lệ" },
                { value: "invalid", label: "Chỉ dòng lỗi" },
              ],
            },
            {
              key: "role",
              label: "Vai trò",
              options: [
                { value: "member", label: "Member" },
                { value: "operator", label: "Operator" },
                { value: "admin", label: "Admin" },
              ],
            },
          ]}
          filterPredicate={(item, activeFilters) => {
            if (activeFilters.validity === "valid" && !item.isValid)
              return false;
            if (activeFilters.validity === "invalid" && item.isValid)
              return false;
            if (activeFilters.role && item.role !== activeFilters.role)
              return false;
            return true;
          }}
          bulkActions={[
            {
              label: "Đổi thành Member",
              action: (items) => handleBulkChangeRole(items, "member"),
            },
            {
              label: "Đổi thành Operator",
              action: (items) => handleBulkChangeRole(items, "operator"),
            },
            {
              label: "Đổi thành Admin",
              action: (items) => handleBulkChangeRole(items, "admin"),
            },
          ]}
        />
      </Panel>

      {/* Execution Action Footer */}
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          justifyContent: "space-between",
          gap: spacing[3],
          padding: spacing[3],
          background: colors.surface.default,
          borderRadius: 8,
          border: `1px solid ${colors.border.default}`,
        }}
      >
        <div style={{ fontSize: 13 }}>
          Tổng cộng: <strong>{validRows.length} tài khoản hợp lệ</strong> sẽ
          được cấp quyền vào <strong>{workspaceName}</strong>.
          {errorRows.length > 0 && (
            <span style={{ color: colors.semantic.danger.text, marginLeft: 8 }}>
              ({errorRows.length} dòng lỗi sẽ tự động được bỏ qua).
            </span>
          )}
        </div>

        <div style={{ display: "flex", gap: spacing[2] }}>
          <Button variant="secondary" onClick={onBack}>
            Hủy bỏ
          </Button>
          <Button
            variant="primary"
            disabled={validRows.length === 0 || isProvisioning}
            loading={isProvisioning}
            onClick={() => void handleExecuteProvision()}
          >
            {isProvisioning
              ? "Đang khởi tạo tài khoản…"
              : `Khởi tạo ${validRows.length} thành viên ngay`}
          </Button>
        </div>
      </div>
    </div>
  );
}
