import { useId, useState, type CSSProperties, type InputHTMLAttributes, type TextareaHTMLAttributes, type FocusEvent } from 'react';
import { colors, focusRing, shape, spacing, typography } from '../tokens';

type ControlProps = InputHTMLAttributes<HTMLInputElement> & TextareaHTMLAttributes<HTMLTextAreaElement>;
export type FieldProps = Omit<ControlProps, 'id'> & { label: string; error?: string; help?: string; required?: boolean; multiline?: boolean; id?: string };
export function Field({ label, error, help, required, multiline = false, id: providedId, style, onFocus, onBlur, ...props }: FieldProps) {
  const generatedId = useId();
  const id = providedId ?? `field-${generatedId}`;
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const describedBy = [help ? hintId : '', error ? errorId : ''].filter(Boolean).join(' ') || undefined;
  const [focused, setFocused] = useState(false);
  const handleFocus = (event: FocusEvent<HTMLInputElement> | FocusEvent<HTMLTextAreaElement>) => { setFocused(true); onFocus?.(event as never); };
  const handleBlur = (event: FocusEvent<HTMLInputElement> | FocusEvent<HTMLTextAreaElement>) => { setFocused(false); onBlur?.(event as never); };
  const controlStyle: CSSProperties = { background: colors.surface.default, border: `1px solid ${error ? colors.semantic.danger.border : colors.border.default}`, borderRadius: shape.sm, boxSizing: 'border-box', color: colors.text.primary, fontFamily: typography.family, fontSize: 14, lineHeight: '20px', minHeight: 34, padding: `${spacing[1.5]}px ${spacing[2]}px`, width: '100%', ...focusRing(focused), ...style };
  const shared = { ...props, 'aria-describedby': describedBy, 'aria-invalid': error ? true : undefined, 'aria-required': required, id, required, onBlur: handleBlur, onFocus: handleFocus, style: controlStyle };
  return <div style={{ display: 'grid', gap: spacing[1], fontFamily: typography.family }}><label htmlFor={id} style={{ ...typography.label, color: colors.text.primary }}>{label}{required ? <span aria-hidden="true"> *</span> : null}</label>{multiline ? <textarea {...shared} rows={4} /> : <input {...shared} />}{help ? <div id={hintId} style={{ ...typography.caption, color: colors.text.muted }}>{help}</div> : null}{error ? <div id={errorId} role="alert" style={{ ...typography.bodySmall, color: colors.semantic.danger.text }}>{error}</div> : null}</div>;
}

