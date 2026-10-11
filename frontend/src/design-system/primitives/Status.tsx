import type { CSSProperties } from 'react';
import { colors, shape, spacing, typography } from '../tokens';
export type StatusTone = 'success' | 'warning' | 'danger' | 'info' | 'neutral';
export type StatusProps = { label: string; tone?: StatusTone; live?: boolean; style?: CSSProperties };
export function Status({ label, tone = 'neutral', live = false, style }: StatusProps) {
  const palette = colors.semantic[tone];
  return <span role={live ? 'status' : undefined} style={{ alignItems: 'center', background: palette.soft, border: `1px solid ${palette.border}`, borderRadius: shape.full, color: palette.text, display: 'inline-flex', fontFamily: typography.family, fontSize: 12, fontWeight: 600, gap: spacing[1], lineHeight: '16px', padding: `2px ${spacing[2]}px`, ...style }}><span aria-hidden="true" style={{ background: palette.strong, borderRadius: shape.full, display: 'inline-block', height: 6, width: 6 }} />{label}</span>;
}
