export const MAX_BULK_INVITATIONS = 100;

export interface BulkInvitationEmailAnalysis {
  emails: string[];
  invalid: string[];
  duplicates: string[];
  exceedsLimit: boolean;
  entryCount: number;
}

export interface BulkAccountRow {
  line: number;
  username: string;
  email: string;
}

export interface DirectAccountRow extends BulkAccountRow {
  initialPassword: string;
}

export function safeProvisioningErrorCode(error: unknown): string | null {
  if (!error || typeof error !== "object" || !("code" in error)) return null;
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" &&
    /^[A-Za-z0-9][A-Za-z0-9_]{0,63}$/.test(code)
    ? code
    : null;
}

export function analyzeDirectAccountRows(input: string): {
  rows: DirectAccountRow[];
  invalidLines: number[];
  duplicateLines: number[];
  exceedsLimit: boolean;
  entryCount: number;
} {
  const entries = input
    .split(/\r?\n/)
    .map((text, index) => ({ line: index + 1, text }))
    .filter(
      ({ text }) =>
        text.trim() &&
        !text.trimStart().startsWith("#") &&
        !/^username\s*,\s*email\s*,\s*initialpassword$/i.test(text.trim()),
    );
  const rows: DirectAccountRow[] = [];
  const invalidLines: number[] = [];
  const duplicateLines: number[] = [];
  const usernames = new Set<string>();
  const emails = new Set<string>();

  for (const { line, text } of entries) {
    const firstComma = text.indexOf(",");
    const secondComma = firstComma < 0 ? -1 : text.indexOf(",", firstComma + 1);
    if (firstComma < 0 || secondComma < 0) {
      invalidLines.push(line);
      continue;
    }
    const username = text.slice(0, firstComma).trim();
    const email = text.slice(firstComma + 1, secondComma).trim();
    const initialPassword = text.slice(secondComma + 1);
    const passwordLength = Array.from(initialPassword).length;
    if (
      username.length < 3 ||
      username.length > 64 ||
      email.length > 254 ||
      !EMAIL_PATTERN.test(email) ||
      passwordLength < MIN_INITIAL_PASSWORD_LENGTH ||
      passwordLength > MAX_INITIAL_PASSWORD_LENGTH
    ) {
      invalidLines.push(line);
      continue;
    }
    const usernameKey = username.toLocaleLowerCase("en-US");
    const emailKey = email.toLocaleLowerCase("en-US");
    if (usernames.has(usernameKey) || emails.has(emailKey)) {
      duplicateLines.push(line);
      continue;
    }
    usernames.add(usernameKey);
    emails.add(emailKey);
    rows.push({ line, username, email, initialPassword });
  }

  return {
    rows,
    invalidLines,
    duplicateLines,
    exceedsLimit: entries.length > MAX_BULK_INVITATIONS,
    entryCount: entries.length,
  };
}

export function analyzeBulkAccountRows(input: string): {
  rows: BulkAccountRow[];
  invalidLines: number[];
  duplicateLines: number[];
  exceedsLimit: boolean;
  entryCount: number;
} {
  const entries = input
    .split(/\r?\n/)
    .map((text, index) => ({
      line: index + 1,
      text: text.trim(),
    }))
    .filter(
      ({ text }) =>
        text && !text.startsWith("#") && !/^username\s*,\s*email$/i.test(text),
    );
  const rows: BulkAccountRow[] = [];
  const invalidLines: number[] = [];
  const duplicateLines: number[] = [];
  const usernames = new Set<string>();
  const emails = new Set<string>();
  for (const { line, text } of entries) {
    const columns = text.split(",").map((part) => part.trim());
    if (
      columns.length !== 2 ||
      columns[0].length < 3 ||
      columns[0].length > 64 ||
      columns[1].length > 254 ||
      !EMAIL_PATTERN.test(columns[1])
    ) {
      invalidLines.push(line);
      continue;
    }
    const usernameKey = columns[0].toLocaleLowerCase("en-US");
    const emailKey = columns[1].toLocaleLowerCase("en-US");
    if (usernames.has(usernameKey) || emails.has(emailKey)) {
      duplicateLines.push(line);
      continue;
    }
    usernames.add(usernameKey);
    emails.add(emailKey);
    rows.push({ line, username: columns[0], email: columns[1] });
  }
  return {
    rows,
    invalidLines,
    duplicateLines,
    exceedsLimit: entries.length > MAX_BULK_INVITATIONS,
    entryCount: entries.length,
  };
}

const EMAIL_PATTERN = /^[^\s@,;]+@[^\s@,;]+\.[^\s@,;]+$/;
const MIN_INITIAL_PASSWORD_LENGTH = 12;
const MAX_INITIAL_PASSWORD_LENGTH = 256;

export function analyzeBulkInvitationEmails(
  input: string,
): BulkInvitationEmailAnalysis {
  const entries = input
    .split(/[\n,;]+/)
    .map((email) => email.trim())
    .filter(Boolean);
  const emails: string[] = [];
  const invalid: string[] = [];
  const duplicates: string[] = [];
  const seen = new Set<string>();

  for (const email of entries) {
    if (email.length > 254 || !EMAIL_PATTERN.test(email)) {
      invalid.push(email);
      continue;
    }
    const canonical = email.toLocaleLowerCase("en-US");
    if (seen.has(canonical)) {
      duplicates.push(email);
      continue;
    }
    seen.add(canonical);
    emails.push(email);
  }

  return {
    emails,
    invalid,
    duplicates,
    exceedsLimit: entries.length > MAX_BULK_INVITATIONS,
    entryCount: entries.length,
  };
}
