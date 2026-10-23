import { type CSSProperties, type ReactNode, useId } from "react";
import { colors, spacing, typography } from "../tokens";
export type TableColumn<Row> = {
  key: string;
  header: string;
  render: (row: Row) => ReactNode;
  width?: string;
  align?: "left" | "center" | "right";
};
export type TableProps<Row> = {
  columns: TableColumn<Row>[];
  rows: Row[];
  getRowKey: (row: Row, index: number) => string;
  caption: string;
  loading?: boolean;
  error?: string;
  emptyMessage?: string;
  onRetry?: () => void;
  style?: CSSProperties;
};
export function Table<Row>({
  columns,
  rows,
  getRowKey,
  caption,
  loading = false,
  error,
  emptyMessage = "No records found.",
  onRetry,
  style,
}: TableProps<Row>) {
  const tableId = useId();
  const statusId = `${tableId}-status`;
  const cellStyle: CSSProperties = {
    borderBottom: `1px solid ${colors.border.subtle}`,
    padding: `${spacing[2]}px ${spacing[3]}px`,
    textAlign: "left",
    ...typography.bodySmall,
  };
  const message = loading
    ? "Loading records…"
    : (error ?? (rows.length === 0 ? emptyMessage : undefined));
  return (
    <div style={{ overflowX: "auto", ...style }}>
      <table
        aria-busy={loading}
        aria-describedby={message ? statusId : undefined}
        style={{
          borderCollapse: "collapse",
          color: colors.text.primary,
          fontFamily: typography.family,
          minWidth: 560,
          width: "100%",
        }}
      >
        <caption
          style={{
            height: 1,
            overflow: "hidden",
            position: "absolute",
            width: 1,
          }}
        >
          {caption}
        </caption>
        <thead>
          <tr>
            {columns.map((column) => (
              <th
                key={column.key}
                scope="col"
                style={{
                  ...cellStyle,
                  background: colors.surface.subtle,
                  color: colors.text.secondary,
                  fontWeight: 600,
                  textAlign: column.align,
                  width: column.width,
                }}
              >
                {column.header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr key={getRowKey(row, index)}>
              {columns.map((column) => (
                <td
                  key={column.key}
                  style={{ ...cellStyle, textAlign: column.align }}
                >
                  {column.render(row)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {message ? (
        <div
          id={statusId}
          aria-live="polite"
          style={{
            alignItems: "center",
            color: error ? colors.semantic.danger.text : colors.text.secondary,
            display: "flex",
            gap: spacing[2],
            padding: `${spacing[3]}px 0`,
            ...typography.bodySmall,
          }}
        >
          {message}
          {error && onRetry ? (
            <button
              type="button"
              onClick={onRetry}
              style={{
                background: "none",
                border: 0,
                color: colors.brand[700],
                cursor: "pointer",
                font: "inherit",
                fontWeight: 600,
                padding: 0,
                textDecoration: "underline",
              }}
            >
              Retry
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
