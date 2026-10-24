import type {
  ProfileGroupsReplaceInput,
  ProfileRecord,
} from "../../../frontend/src/generated/command-registry.ts";

type ProfileLookup = Pick<ProfileRecord, "id" | "name" | "revision">;

export type ProfileGroupBulkApi = {
  getProfile(input: { profileId: string }): Promise<ProfileLookup>;
  replaceProfileGroups(
    input: ProfileGroupsReplaceInput,
  ): Promise<Pick<ProfileRecord, "id" | "groupIds">>;
};

export type ProfileGroupBulkFailure = {
  profileId: string;
  name: string;
  error: string;
};

export type ProfileGroupBulkResult = {
  succeededIds: string[];
  failures: ProfileGroupBulkFailure[];
};

export async function replaceGroupsForProfiles(input: {
  api: ProfileGroupBulkApi;
  workspaceId: string;
  profiles: Array<{ id: string; name: string }>;
  groupIds: string[];
}): Promise<ProfileGroupBulkResult> {
  const result: ProfileGroupBulkResult = { succeededIds: [], failures: [] };
  for (const resource of input.profiles) {
    try {
      const profile = await input.api.getProfile({ profileId: resource.id });
      await input.api.replaceProfileGroups({
        workspaceId: input.workspaceId,
        profileId: profile.id,
        expectedRevision: profile.revision,
        groupIds: input.groupIds,
      });
      result.succeededIds.push(resource.id);
    } catch (error) {
      result.failures.push({
        profileId: resource.id,
        name: resource.name,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }
  return result;
}
