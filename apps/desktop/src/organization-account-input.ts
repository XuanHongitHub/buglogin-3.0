export interface OrganizationAccountRow {
  line: number;
  username: string;
  email: string;
  password: string;
}

export interface OrganizationAccountBulkAnalysis {
  rows: OrganizationAccountRow[];
  invalidLines: number[];
  duplicateLines: number[];
  exceedsLimit: boolean;
  entryCount: number;
}

const MAX_ROWS = 100;
const MIN_PASSWORD_LENGTH = 12;
const MAX_PASSWORD_LENGTH = 256;
const passwordLength = (value: string) => Array.from(value).length;
const emailValid = (value: string) =>
  /^[^\s@,;]+@[^\s@,;]+\.[^\s@,;]+$/.test(value);

export function analyzeOrganizationAccountRows(
  input: string,
): OrganizationAccountBulkAnalysis {
  const entries = input
    .split(/\r?\n/)
    .map((text, index) => ({ line: index + 1, text }))
    .filter(({ text }) => {
      const value = text.trim();
      return (
        value &&
        !value.startsWith("#") &&
        !/^username\s*,\s*email\s*,\s*initial[-_ ]?password$/i.test(value)
      );
    });
  const rows: OrganizationAccountRow[] = [];
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
    const password = text.slice(secondComma + 1);
    if (
      username.length < 3 ||
      username.length > 64 ||
      email.length > 254 ||
      !emailValid(email) ||
      passwordLength(password) < MIN_PASSWORD_LENGTH ||
      passwordLength(password) > MAX_PASSWORD_LENGTH
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
    rows.push({ line, username, email, password });
  }

  return {
    rows,
    invalidLines,
    duplicateLines,
    exceedsLimit: entries.length > MAX_ROWS,
    entryCount: entries.length,
  };
}

export function validateOrganizationAccountFields(input: {
  username: string;
  email: string;
  password: string;
  confirmPassword: string;
}): string | null {
  const username = input.username.trim();
  const email = input.email.trim();
  if (username.length < 3 || username.length > 64) return "invalid_username";
  if (email.length > 254 || !emailValid(email)) return "invalid_email";
  if (
    passwordLength(input.password) < MIN_PASSWORD_LENGTH ||
    passwordLength(input.password) > MAX_PASSWORD_LENGTH
  )
    return "invalid_password_length";
  if (input.password !== input.confirmPassword) return "password_mismatch";
  return null;
}
