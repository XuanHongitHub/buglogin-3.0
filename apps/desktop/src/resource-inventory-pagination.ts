import type { ResourceListInput } from "../../../frontend/src/generated/command-registry.ts";

export const RESOURCE_INVENTORY_PAGE_SIZE = 50;

export type ResourceCursorPage = { cursor?: string };

export function resourceListPageInput(
  workspaceId: string,
  cursor?: string,
): ResourceListInput {
  return {
    workspaceId,
    limit: RESOURCE_INVENTORY_PAGE_SIZE,
    ...(cursor ? { cursor } : {}),
  };
}

export function appendResourceCursorPage(
  stack: ResourceCursorPage[],
  currentPageIndex: number,
  nextCursor: string | null | undefined,
): ResourceCursorPage[] | null {
  if (!nextCursor || stack.some((page) => page.cursor === nextCursor))
    return null;
  return [...stack.slice(0, currentPageIndex + 1), { cursor: nextCursor }];
}
