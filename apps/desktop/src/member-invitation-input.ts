export const MAX_BULK_INVITATIONS = 100;

export interface BulkInvitationEmailAnalysis {
  emails: string[];
  invalid: string[];
  duplicates: string[];
  exceedsLimit: boolean;
  entryCount: number;
}

const EMAIL_PATTERN = /^[^\s@,;]+@[^\s@,;]+\.[^\s@,;]+$/;

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
