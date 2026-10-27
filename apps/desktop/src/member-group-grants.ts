import type { GroupShareGrant } from "../../../frontend/src/generated/command-registry.ts";

export type DirectGroupGrant = {
  groupId: string;
  action: GroupShareGrant["action"];
};
export type DirectGroupGrantMode = "add" | "replace" | "remove";

export function previewDirectGroupGrants(
  current: DirectGroupGrant[],
  mode: DirectGroupGrantMode,
  selectedGroupIds: string[],
): DirectGroupGrant[] {
  let next = current.map((grant) => ({ ...grant }));
  if (mode === "replace")
    next = next.filter((grant) => selectedGroupIds.includes(grant.groupId));
  else if (mode === "remove")
    next = next.filter((grant) => !selectedGroupIds.includes(grant.groupId));

  if (mode !== "remove")
    for (const groupId of selectedGroupIds)
      for (const action of ["read", "execute"] as const)
        if (
          !next.some(
            (grant) => grant.groupId === groupId && grant.action === action,
          )
        )
          next.push({ groupId, action });

  return next.sort((left, right) =>
    `${left.groupId}:${left.action}`.localeCompare(
      `${right.groupId}:${right.action}`,
    ),
  );
}
