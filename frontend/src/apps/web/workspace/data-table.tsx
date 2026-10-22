import { type ReactNode, useMemo, useState } from "react";
import { Button, colors, spacing } from "../../../design-system";

export interface Column<T> {
  key: string;
  header: string;
  sortable?: boolean;
  sortValue?: (item: T) => string | number;
  render: (item: T) => ReactNode;
  width?: string | number;
}

export interface FilterOption {
  key: string;
  label: string;
  options: { value: string; label: string }[];
}

interface EnterpriseDataTableProps<T> {
  data: T[];
  columns: Column<T>[];
  getRowKey: (item: T) => string;
  searchPlaceholder?: string;
  searchFilter?: (item: T, needle: string) => boolean;
  filters?: FilterOption[];
  filterPredicate?: (item: T, activeFilters: Record<string, string>) => boolean;
  bulkActions?: {
    label: string;
    action: (selectedItems: T[]) => void | Promise<void>;
    variant?: "primary" | "secondary" | "danger";
  }[];
  emptyMessage?: string;
  defaultPageSize?: number;
  pageSizeOptions?: number[];
  actionsHeader?: ReactNode;
}

export function EnterpriseDataTable<T>({
  data,
  columns,
  getRowKey,
  searchPlaceholder = "Tìm kiếm...",
  searchFilter,
  filters = [],
  filterPredicate,
  bulkActions = [],
  emptyMessage = "Không có dữ liệu phù hợp.",
  defaultPageSize = 10,
  pageSizeOptions = [10, 25, 50, 100],
  actionsHeader,
}: EnterpriseDataTableProps<T>) {
  const [search, setSearch] = useState("");
  const [activeFilters, setActiveFilters] = useState<Record<string, string>>(
    {},
  );
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(defaultPageSize);
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());

  // 1. Search & Filter
  const filteredData = useMemo(() => {
    let result = data;
    const needle = search.trim().toLowerCase();
    if (needle && searchFilter) {
      result = result.filter((item) => searchFilter(item, needle));
    }
    if (filterPredicate && Object.keys(activeFilters).length > 0) {
      result = result.filter((item) => filterPredicate(item, activeFilters));
    }
    return result;
  }, [data, search, searchFilter, activeFilters, filterPredicate]);

  // 2. Sorting
  const sortedData = useMemo(() => {
    if (!sortKey) return filteredData;
    const col = columns.find((c) => c.key === sortKey);
    if (!col?.sortValue) return filteredData;

    return [...filteredData].sort((a, b) => {
      const valA = col.sortValue?.(a);
      const valB = col.sortValue?.(b);
      let cmp = 0;
      if (typeof valA === "number" && typeof valB === "number") {
        cmp = valA - valB;
      } else {
        cmp = String(valA).localeCompare(String(valB));
      }
      return sortDirection === "asc" ? cmp : -cmp;
    });
  }, [filteredData, sortKey, sortDirection, columns]);

  // 3. Pagination calculation
  const totalItems = sortedData.length;
  const totalPages = Math.max(1, Math.ceil(totalItems / pageSize));
  const safeCurrentPage = Math.min(currentPage, totalPages);
  const startIndex = (safeCurrentPage - 1) * pageSize;
  const endIndex = Math.min(startIndex + pageSize, totalItems);
  const pageRows = useMemo(() => {
    return sortedData.slice(startIndex, endIndex);
  }, [sortedData, startIndex, endIndex]);

  const handleSort = (key: string) => {
    if (sortKey === key) {
      setSortDirection((prev) => (prev === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDirection("asc");
    }
    setCurrentPage(1);
  };

  const handleSelectAll = (checked: boolean) => {
    if (checked) {
      const keys = new Set(pageRows.map(getRowKey));
      setSelectedKeys(keys);
    } else {
      setSelectedKeys(new Set());
    }
  };

  const handleSelectRow = (key: string, checked: boolean) => {
    setSelectedKeys((prev) => {
      const next = new Set(prev);
      if (checked) next.add(key);
      else next.delete(key);
      return next;
    });
  };

  const selectedItems = useMemo(() => {
    return data.filter((item) => selectedKeys.has(getRowKey(item)));
  }, [data, selectedKeys, getRowKey]);

  const isAllPageSelected =
    pageRows.length > 0 &&
    pageRows.every((r) => selectedKeys.has(getRowKey(r)));

  return (
    <div style={{ display: "grid", gap: spacing[3], width: "100%" }}>
      {/* Top Toolbar: Search, Filters, Actions */}
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          justifyContent: "space-between",
          gap: spacing[2],
          padding: spacing[2],
          background: colors.surface.subtle,
          borderRadius: 6,
          border: `1px solid ${colors.border.subtle}`,
        }}
      >
        <div
          style={{
            display: "flex",
            flexWrap: "wrap",
            alignItems: "center",
            gap: spacing[2],
            flex: 1,
          }}
        >
          {/* Search Box */}
          <div
            style={{
              position: "relative",
              minWidth: 220,
              maxWidth: 360,
              flex: 1,
            }}
          >
            <input
              type="text"
              value={search}
              onChange={(e) => {
                setSearch(e.target.value);
                setCurrentPage(1);
              }}
              placeholder={searchPlaceholder}
              style={{
                width: "100%",
                height: 32,
                padding: "0 28px 0 10px",
                fontSize: 13,
                borderRadius: 4,
                border: `1px solid ${colors.border.default}`,
                background: colors.surface.default,
                color: colors.text.primary,
                outline: "none",
              }}
            />
            {search && (
              <button
                type="button"
                onClick={() => setSearch("")}
                style={{
                  position: "absolute",
                  right: 8,
                  top: "50%",
                  transform: "translateY(-50%)",
                  border: "none",
                  background: "transparent",
                  color: colors.text.muted,
                  cursor: "pointer",
                  fontSize: 13,
                }}
              >
                ✕
              </button>
            )}
          </div>

          {/* Filter Dropdowns */}
          {filters.map((filter) => (
            <select
              key={filter.key}
              aria-label={filter.label}
              value={activeFilters[filter.key] || ""}
              onChange={(e) => {
                const val = e.target.value;
                setActiveFilters((prev) => {
                  const next = { ...prev };
                  if (val) next[filter.key] = val;
                  else delete next[filter.key];
                  return next;
                });
                setCurrentPage(1);
              }}
              style={{
                height: 32,
                padding: "0 8px",
                fontSize: 12,
                borderRadius: 4,
                border: `1px solid ${colors.border.default}`,
                background: colors.surface.default,
                color: colors.text.primary,
                cursor: "pointer",
              }}
            >
              <option value="">{filter.label}: Tất cả</option>
              {filter.options.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </select>
          ))}

          {/* Reset Filters button if any active */}
          {(search || Object.keys(activeFilters).length > 0) && (
            <button
              type="button"
              onClick={() => {
                setSearch("");
                setActiveFilters({});
                setCurrentPage(1);
              }}
              style={{
                border: "none",
                background: "transparent",
                color: colors.brand[600],
                fontSize: 12,
                cursor: "pointer",
                padding: "4px 8px",
                textDecoration: "underline",
              }}
            >
              Đặt lại bộ lọc
            </button>
          )}
        </div>

        {/* Custom Actions in Toolbar Header */}
        {actionsHeader && <div>{actionsHeader}</div>}
      </div>

      {/* Bulk Action Bar (when rows selected) */}
      {selectedKeys.size > 0 && bulkActions.length > 0 && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "8px 12px",
            background: colors.surface.selected,
            borderRadius: 6,
            border: `1px solid ${colors.brand[500]}`,
          }}
        >
          <span
            style={{ fontSize: 13, fontWeight: 600, color: colors.brand[700] }}
          >
            Đã chọn {selectedKeys.size} bản ghi
          </span>
          <div style={{ display: "flex", gap: spacing[2] }}>
            {bulkActions.map((bulkAction) => (
              <Button
                key={bulkAction.label}
                variant={bulkAction.variant || "secondary"}
                onClick={() => void bulkAction.action(selectedItems)}
              >
                {bulkAction.label}
              </Button>
            ))}
            <Button
              variant="secondary"
              onClick={() => setSelectedKeys(new Set())}
            >
              Bỏ chọn
            </Button>
          </div>
        </div>
      )}

      {/* Main Table */}
      <div
        style={{
          overflowX: "auto",
          border: `1px solid ${colors.border.subtle}`,
          borderRadius: 6,
        }}
      >
        <table
          style={{
            width: "100%",
            borderCollapse: "collapse",
            fontSize: 13,
            textAlign: "left",
          }}
        >
          <thead>
            <tr
              style={{
                background: colors.surface.sunken,
                borderBottom: `1px solid ${colors.border.subtle}`,
              }}
            >
              {bulkActions.length > 0 && (
                <th
                  style={{
                    width: 40,
                    padding: "10px 12px",
                    textAlign: "center",
                  }}
                >
                  <input
                    type="checkbox"
                    checked={isAllPageSelected}
                    onChange={(e) => handleSelectAll(e.target.checked)}
                    aria-label="Chọn tất cả hàng trên trang"
                  />
                </th>
              )}
              {columns.map((col) => (
                <th
                  key={col.key}
                  onClick={() => col.sortable && handleSort(col.key)}
                  style={{
                    padding: "10px 14px",
                    fontWeight: 650,
                    color: colors.text.primary,
                    cursor: col.sortable ? "pointer" : "default",
                    userSelect: "none",
                    width: col.width,
                  }}
                >
                  <div
                    style={{
                      display: "inline-flex",
                      alignItems: "center",
                      gap: 4,
                    }}
                  >
                    <span>{col.header}</span>
                    {col.sortable && (
                      <span
                        style={{
                          fontSize: 10,
                          color:
                            sortKey === col.key
                              ? colors.brand[600]
                              : colors.text.muted,
                        }}
                      >
                        {sortKey === col.key
                          ? sortDirection === "asc"
                            ? "▲"
                            : "▼"
                          : "⇅"}
                      </span>
                    )}
                  </div>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {pageRows.length === 0 ? (
              <tr>
                <td
                  colSpan={columns.length + (bulkActions.length > 0 ? 1 : 0)}
                  style={{
                    padding: "36px 16px",
                    textAlign: "center",
                    color: colors.text.muted,
                  }}
                >
                  {emptyMessage}
                </td>
              </tr>
            ) : (
              pageRows.map((row) => {
                const key = getRowKey(row);
                const isSelected = selectedKeys.has(key);
                return (
                  <tr
                    key={key}
                    style={{
                      borderBottom: `1px solid ${colors.border.subtle}`,
                      background: isSelected
                        ? colors.surface.selected
                        : colors.surface.default,
                      transition: "background 0.12s ease",
                    }}
                  >
                    {bulkActions.length > 0 && (
                      <td style={{ padding: "10px 12px", textAlign: "center" }}>
                        <input
                          type="checkbox"
                          checked={isSelected}
                          onChange={(e) =>
                            handleSelectRow(key, e.target.checked)
                          }
                          aria-label={`Chọn hàng ${key}`}
                        />
                      </td>
                    )}
                    {columns.map((col) => (
                      <td
                        key={col.key}
                        style={{
                          padding: "10px 14px",
                          verticalAlign: "middle",
                        }}
                      >
                        {col.render(row)}
                      </td>
                    ))}
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>

      {/* Pagination Footer */}
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          justifyContent: "space-between",
          gap: spacing[2],
          padding: "8px 4px",
          fontSize: 12,
          color: colors.text.muted,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <span>
            Hiển thị {totalItems === 0 ? 0 : startIndex + 1} - {endIndex} trên{" "}
            {totalItems.toLocaleString()} bản ghi
          </span>
          <span style={{ color: colors.border.strong }}>|</span>
          <label
            style={{ display: "inline-flex", alignItems: "center", gap: 4 }}
          >
            <span>Số hàng:</span>
            <select
              value={pageSize}
              onChange={(e) => {
                setPageSize(Number(e.target.value));
                setCurrentPage(1);
              }}
              style={{
                height: 26,
                padding: "0 4px",
                fontSize: 12,
                borderRadius: 4,
                border: `1px solid ${colors.border.default}`,
                background: colors.surface.default,
                color: colors.text.primary,
              }}
            >
              {pageSizeOptions.map((opt) => (
                <option key={opt} value={opt}>
                  {opt}
                </option>
              ))}
            </select>
          </label>
        </div>

        {/* Page Navigators */}
        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
          <button
            type="button"
            disabled={safeCurrentPage <= 1}
            onClick={() => setCurrentPage(1)}
            style={{
              padding: "4px 8px",
              borderRadius: 4,
              border: `1px solid ${colors.border.default}`,
              background: colors.surface.default,
              cursor: safeCurrentPage <= 1 ? "not-allowed" : "pointer",
              opacity: safeCurrentPage <= 1 ? 0.5 : 1,
              fontSize: 12,
            }}
          >
            « Đầu
          </button>
          <button
            type="button"
            disabled={safeCurrentPage <= 1}
            onClick={() => setCurrentPage((p) => Math.max(1, p - 1))}
            style={{
              padding: "4px 8px",
              borderRadius: 4,
              border: `1px solid ${colors.border.default}`,
              background: colors.surface.default,
              cursor: safeCurrentPage <= 1 ? "not-allowed" : "pointer",
              opacity: safeCurrentPage <= 1 ? 0.5 : 1,
              fontSize: 12,
            }}
          >
            ‹ Trước
          </button>
          <span
            style={{
              padding: "0 8px",
              fontWeight: 600,
              color: colors.text.primary,
            }}
          >
            Trang {safeCurrentPage} / {totalPages}
          </span>
          <button
            type="button"
            disabled={safeCurrentPage >= totalPages}
            onClick={() => setCurrentPage((p) => Math.min(totalPages, p + 1))}
            style={{
              padding: "4px 8px",
              borderRadius: 4,
              border: `1px solid ${colors.border.default}`,
              background: colors.surface.default,
              cursor: safeCurrentPage >= totalPages ? "not-allowed" : "pointer",
              opacity: safeCurrentPage >= totalPages ? 0.5 : 1,
              fontSize: 12,
            }}
          >
            Sau ›
          </button>
          <button
            type="button"
            disabled={safeCurrentPage >= totalPages}
            onClick={() => setCurrentPage(totalPages)}
            style={{
              padding: "4px 8px",
              borderRadius: 4,
              border: `1px solid ${colors.border.default}`,
              background: colors.surface.default,
              cursor: safeCurrentPage >= totalPages ? "not-allowed" : "pointer",
              opacity: safeCurrentPage >= totalPages ? 0.5 : 1,
              fontSize: 12,
            }}
          >
            Cuối »
          </button>
        </div>
      </div>
    </div>
  );
}
