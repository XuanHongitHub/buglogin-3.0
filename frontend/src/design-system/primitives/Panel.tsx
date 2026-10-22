import type { HTMLAttributes, ReactNode } from "react";
import { colors, shape, spacing, typography } from "../tokens";
export type PanelProps = HTMLAttributes<HTMLElement> & {
  title?: string;
  description?: string;
  actions?: ReactNode;
};
export function Panel({
  title,
  description,
  actions,
  children,
  style,
  ...props
}: PanelProps) {
  return (
    <section
      {...props}
      style={{
        background: colors.surface.default,
        border: `1px solid ${colors.border.default}`,
        borderRadius: shape.md,
        color: colors.text.primary,
        fontFamily: typography.family,
        padding: spacing[4],
        ...style,
      }}
    >
      {title || description || actions ? (
        <header
          style={{
            alignItems: "start",
            borderBottom: children
              ? `1px solid ${colors.border.subtle}`
              : undefined,
            display: "flex",
            gap: spacing[3],
            justifyContent: "space-between",
            marginBottom: children ? spacing[4] : 0,
            paddingBottom: children ? spacing[3] : 0,
          }}
        >
          <div>
            {title ? (
              <h2 style={{ margin: 0, ...typography.heading }}>{title}</h2>
            ) : null}
            {description ? (
              <p
                style={{
                  color: colors.text.secondary,
                  margin: `${spacing[1]}px 0 0`,
                  ...typography.bodySmall,
                }}
              >
                {description}
              </p>
            ) : null}
          </div>
          {actions ? (
            <div
              style={{ alignItems: "center", display: "flex", gap: spacing[2] }}
            >
              {actions}
            </div>
          ) : null}
        </header>
      ) : null}
      {children}
    </section>
  );
}
