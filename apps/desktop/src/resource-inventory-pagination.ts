import type {
  ResourceKind,
  ResourceListInput,
  ResourceState,
} from "../../../frontend/src/generated/command-registry.ts";

export const RESOURCE_INVENTORY_PAGE_SIZE = 50;

export type ResourceCursorPage = { cursor?: string };

export function resourceListPageInput(
  workspaceId: string,
  cursor?: string,
  filters: {
    search?: string;
    kind?: ResourceKind;
    state?: ResourceState;
  } = {},
): ResourceListInput {
  return {
    workspaceId,
    limit: RESOURCE_INVENTORY_PAGE_SIZE,
    ...(cursor ? { cursor } : {}),
    ...(filters.search ? { search: filters.search } : {}),
    ...(filters.kind ? { kind: filters.kind } : {}),
    ...(filters.state ? { state: filters.state } : {}),
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
