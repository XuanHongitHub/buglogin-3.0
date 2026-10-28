export type AccountImportRow = {
  username: string;
  displayName: string;
  email: string;
  initialPassword: string;
};

export type AccountInputIssue =
  | "username"
  | "email"
  | "password"
  | "duplicateUsername"
  | "duplicateEmail";

/** RFC 4180-style quoted cells; tab-separated clipboard data uses the same quoting rules. */
export function parseAccountTable(text: string): AccountImportRow[] {
  const source = text.replace(/^\uFEFF/, "");
  const delimiter = source.split(/\r?\n/, 1)[0]?.includes("\t") ? "\t" : ",";
  const records: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    if (character === '"') {
      if (quoted && source[index + 1] === '"') {
        cell += '"';
        index += 1;
      } else if (quoted || cell.length === 0) quoted = !quoted;
      else cell += character;
    } else if (character === delimiter && !quoted) {
      row.push(cell);
      cell = "";
    } else if ((character === "\n" || character === "\r") && !quoted) {
      if (character === "\r" && source[index + 1] === "\n") index += 1;
      row.push(cell);
      if (row.some((value) => value.trim())) records.push(row);
      row = [];
      cell = "";
    } else cell += character;
  }
  if (quoted) throw new Error("unclosedQuote");
  row.push(cell);
  if (row.some((value) => value.trim())) records.push(row);
  if (records.length === 0) return [];
  const normalized = records[0].map((value) =>
    value.trim().toLowerCase().replace(/[ _-]/g, ""),
  );
  const hasHeader = normalized.includes("username");
  const headers = hasHeader
    ? normalized
    : ["username", "email", "initialpassword", "displayname"];
  const field = (values: string[], names: string[]) => {
    const position = headers.findIndex((header) => names.includes(header));
    return position < 0 ? "" : (values[position] ?? "");
  };
  return records.slice(hasHeader ? 1 : 0).map((values) => ({
    username: field(values, ["username"]).trim(),
    email: field(values, ["email"]).trim(),
    initialPassword: field(values, ["initialpassword", "password"]),
    displayName: field(values, ["displayname", "name"]).trim(),
  }));
}

export function validateAccountRows(
  rows: AccountImportRow[],
): AccountInputIssue[][] {
  const usernameCounts = new Map<string, number>();
  const emailCounts = new Map<string, number>();
  for (const row of rows) {
    const username = row.username.trim().toLowerCase();
    const email = row.email.trim().toLowerCase();
    usernameCounts.set(username, (usernameCounts.get(username) ?? 0) + 1);
    if (email) emailCounts.set(email, (emailCounts.get(email) ?? 0) + 1);
  }
  return rows.map((row) => {
    const errors: AccountInputIssue[] = [];
    const username = row.username.trim();
    const email = row.email.trim();
    if (
      Array.from(username).length < 3 ||
      Array.from(username).length > 64 ||
      /[\s@\u0000-\u001F]/.test(username)
    )
      errors.push("username");
    if (
      email &&
      (email.length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email))
    )
      errors.push("email");
    if (row.initialPassword.length < 4 || row.initialPassword.length > 256)
      errors.push("password");
    if ((usernameCounts.get(username.toLowerCase()) ?? 0) > 1)
      errors.push("duplicateUsername");
    if (email && (emailCounts.get(email.toLowerCase()) ?? 0) > 1)
      errors.push("duplicateEmail");
    return errors;
  });
}

export function toAccountCsv(
  rows: AccountImportRow[],
  includeSecrets: boolean,
): string {
  const cell = (value: string) => `"${value.replaceAll('"', '""')}"`;
  const headings = [
    "username",
    "displayName",
    "email",
    ...(includeSecrets ? ["initialPassword"] : []),
  ];
  return [
    headings.join(","),
    ...rows.map((row) =>
      [
        row.username,
        row.displayName,
        row.email,
        ...(includeSecrets ? [row.initialPassword] : []),
      ]
        .map(cell)
        .join(","),
    ),
  ].join("\r\n");
}
