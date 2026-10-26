export type GroupAssignmentMode = "add" | "remove" | "replace";

export function changeGroupAssignment(
  current: string[],
  chosen: string[],
  mode: GroupAssignmentMode,
): string[] {
  if (mode === "replace") return [...new Set(chosen)];
  if (mode === "remove") return current.filter((id) => !chosen.includes(id));
  return [...new Set([...current, ...chosen])];
}

export function groupNameRows(
  text: string,
): { name: string; line: number; duplicate: boolean }[] {
  const seen = new Set<string>();
  return text.split(/\r?\n/).flatMap((line, index) => {
    const name = line.trim();
    if (!name) return [];
    const canonical = name.normalize("NFKC").toLocaleLowerCase();
    const duplicate = seen.has(canonical);
    seen.add(canonical);
    return [{ name, line: index + 1, duplicate }];
  });
}
