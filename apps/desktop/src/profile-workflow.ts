export function normalizedRequiredText(value: string) {
  const normalized = value.trim();
  return normalized || undefined;
}

export function usableRevision(
  revision: number | undefined,
): revision is number {
  return revision !== undefined && Number.isInteger(revision) && revision > 0;
}
