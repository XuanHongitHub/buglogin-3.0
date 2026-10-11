import { useState, type ButtonHTMLAttributes, type CSSProperties, type FocusEvent } from 'react';
import { colors, focusRing, shape, spacing, typography } from '../tokens';

export type ButtonVariant = 'primary' | 'secondary' | 'quiet' | 'danger';
export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant; loading?: boolean; loadingLabel?: string };
const variants: Record<ButtonVariant, CSSProperties> = {
  primary: { background: colors.brand[600], borderColor: colors.brand[600], color: '#FFFFFF' },
  secondary: { background: colors.surface.default, borderColor: colors.border.default, color: colors.text.primary },
  quiet: { background: 'transparent', borderColor: 'transparent', color: colors.text.secondary },
  danger: { background: colors.semantic.danger.strong, borderColor: colors.semantic.danger.strong, color: '#FFFFFF' },
};
export function Button({ variant = 'secondary', loading = false, loadingLabel, disabled, children, style, onFocus, onBlur, ...props }: ButtonProps) {
  const [focused, setFocused] = useState(false);
  const handleFocus = (event: FocusEvent<HTMLButtonElement>) => { setFocused(true); onFocus?.(event); };
  const handleBlur = (event: FocusEvent<HTMLButtonElement>) => { setFocused(false); onBlur?.(event); };
  const compact = variant === 'quiet';
  return <button {...props} aria-busy={loading || undefined} disabled={disabled || loading} onBlur={handleBlur} onFocus={handleFocus} style={{ alignItems: 'center', border: '1px solid', borderRadius: shape.sm, cursor: disabled || loading ? 'not-allowed' : 'pointer', display: 'inline-flex', fontFamily: typography.family, fontSize: 13, fontWeight: 600, gap: spacing[2], justifyContent: 'center', minHeight: compact ? 28 : 32, opacity: disabled ? 0.55 : 1, padding: `0 ${spacing[3]}px`, transition: 'background-color 120ms ease, border-color 120ms ease, outline 120ms ease', ...variants[variant], ...focusRing(focused), ...style }}>{loading ? (loadingLabel ?? <>{children}<span aria-hidden="true">…</span></>) : children}</button>;
}
