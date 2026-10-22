export const colors = {
  // Typed BugLogin roles resolve through Donut's semantic theme variables.
  // Keep this adapter as the single bridge for React primitives and CSS.
  canvas: {
    default: "var(--background, #FCFCFB)",
    subtle: "var(--muted, #F5F6F7)",
    admin: "var(--sidebar, #F2F3F5)",
  },
  surface: {
    default: "var(--card, #FFFFFF)",
    subtle: "var(--secondary, #F8F9FA)",
    sunken: "var(--muted, #EEF0F2)",
    selected: "var(--accent, #EAF2FF)",
  },
  border: {
    subtle: "var(--border, #E4E7EB)",
    default: "var(--border, #D3D8DE)",
    strong: "var(--input, #AEB6C0)",
  },
  text: {
    primary: "var(--foreground, #17202A)",
    secondary: "var(--muted-foreground, #4F5B67)",
    muted: "var(--muted-foreground, #74808C)",
    disabled: "var(--muted-foreground, #9DA6AF)",
  },
  brand: {
    700: "var(--primary, #174EA6)",
    600: "var(--primary, #1F64D1)",
    500: "var(--accent-foreground, #3478E5)",
    100: "var(--accent, #DCEAFF)",
  },
  semantic: {
    success: {
      strong: "var(--success, #238636)",
      text: "var(--success-foreground, #176B2C)",
      soft: "color-mix(in srgb, var(--success, #238636) 12%, var(--background, #FCFCFB))",
      border: "var(--success, #A9D9B4)",
    },
    warning: {
      strong: "var(--warning, #B66A00)",
      text: "var(--warning-foreground, #8A4F00)",
      soft: "color-mix(in srgb, var(--warning, #B66A00) 16%, var(--background, #FCFCFB))",
      border: "var(--warning, #E8C17A)",
    },
    danger: {
      strong: "var(--destructive, #C73535)",
      text: "var(--destructive-foreground, #9F2525)",
      soft: "color-mix(in srgb, var(--destructive, #C73535) 12%, var(--background, #FCFCFB))",
      border: "var(--destructive, #E5AAAA)",
    },
    info: {
      strong: "var(--primary, #1F64D1)",
      text: "var(--primary, #174EA6)",
      soft: "var(--accent, #EAF2FF)",
      border: "var(--border, #AFCBF5)",
    },
    neutral: {
      strong: "var(--muted-foreground, #697580)",
      text: "var(--muted-foreground, #53606C)",
      soft: "var(--muted, #EEF0F2)",
      border: "var(--border, #CDD2D8)",
    },
  },
  focus: "var(--ring, #2E74D6)",
} as const;

export const spacing = {
  0: 0,
  0.5: 2,
  1: 4,
  1.5: 6,
  2: 8,
  3: 12,
  4: 16,
  5: 20,
  6: 24,
  8: 32,
} as const;
export const shape = {
  xs: "var(--radius-sm, 3px)",
  sm: "var(--radius-sm, 5px)",
  md: "var(--radius-md, 8px)",
  lg: "var(--radius-lg, 12px)",
  full: "999px",
} as const;
export const typography = {
  family: 'var(--font-geist-sans, Geist Sans), "Segoe UI", sans-serif',
  mono: 'var(--font-geist-mono, Geist Mono), "Cascadia Mono", monospace',
  body: { fontSize: 14, lineHeight: "20px", fontWeight: 400 },
  bodySmall: { fontSize: 13, lineHeight: "18px", fontWeight: 400 },
  label: { fontSize: 12, lineHeight: "16px", fontWeight: 600 },
  heading: { fontSize: 18, lineHeight: "24px", fontWeight: 620 },
  caption: { fontSize: 11, lineHeight: "16px", fontWeight: 500 },
} as const;
export const focusRing = (focused: boolean) =>
  focused ? { outline: `2px solid ${colors.focus}`, outlineOffset: 2 } : {};
