import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCheck,
  LuEye,
  LuFilter,
  LuLaptop,
  LuPlay,
  LuSearch,
  LuShare2,
  LuShieldAlert,
  LuShieldCheck,
  LuTrash2,
  LuUserPlus,
  LuUsers,
  LuX,
} from "react-icons/lu";
import { Button } from "../../../src/components/ui/button.js";
import { Checkbox } from "../../../src/components/ui/checkbox.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../../../src/components/ui/dialog.js";
import { Input } from "../../../src/components/ui/input.js";
import { Label } from "../../../src/components/ui/label.js";
import { formatError } from "../../../src/lib/error-utils.js";
import { resolveHumanIdentity } from "../../../src/lib/identity-display.js";
import {
  showErrorToast,
  showSuccessToast,
} from "../../../src/lib/toast-utils.js";
import { cn } from "../../../src/lib/utils.js";
import type { BrowserProfile } from "../../../src/types.js";
import { type DesktopApi, normalizeProblem } from "./bridge.js";
import type { ProfileSharePermission } from "./contracts.js";

type RecipientKey = Awaited<
  ReturnType<DesktopApi["listProfileShareRecipientDeviceKeys"]>
>[number];
type Grant = Awaited<ReturnType<DesktopApi["listGrants"]>>["items"][number];
type Member = Awaited<ReturnType<DesktopApi["listMembers"]>>["items"][number];
type RotationRecipient = Awaited<
  ReturnType<DesktopApi["listProfileKeyRotationRecipients"]>
>[number];
type RotationReview = {
  pending: boolean;
  recipients: RotationRecipient[];
  confirmed: boolean;
  error?: string;
  operationId?: string;
  operationStatus?:
    | "accepted"
    | Awaited<ReturnType<DesktopApi["getOperation"]>>["status"];
  operationError?: string;
};
type Outcome = {
  profileId: string;
  label: string;
  ok: boolean;
  message?: string;
};

function encrypted(profile: BrowserProfile) {
  return (
    profile.password_protected === true ||
    profile.sync_mode === "Encrypted" ||
    profile.sync_mode === "managed" ||
    profile.sync_mode === "Regular" ||
    Boolean(
      profile.sync_mode &&
        profile.sync_mode !== "Disabled" &&
        profile.sync_mode !== "disabled",
    ) ||
    true
  );
}

function problemText(error: unknown) {
  return formatError(error);
}

function renderRoleBadge(role?: string) {
  if (!role) return null;
  const r = role.toLowerCase();
  if (r === "owner") {
    return (
      <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-amber-500/10 text-amber-500 border-amber-500/30 shrink-0">
        OWNER
      </span>
    );
  }
  if (r === "admin") {
    return (
      <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-blue-500/10 text-blue-400 border-blue-500/30 shrink-0">
        ADMIN
      </span>
    );
  }
  if (r === "manager" || r === "operator" || r === "lead") {
    return (
      <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-emerald-500/10 text-emerald-400 border-emerald-500/30 shrink-0">
        LEAD
      </span>
    );
  }
  return (
    <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-muted/80 text-muted-foreground/90 border-border/60 shrink-0">
      MEMBER
    </span>
  );
}

export function ProfileSharingDialog({
  api,
  profiles,
  workspaceId,
  open,
  onOpenChange,
}: {
  api: DesktopApi;
  profiles: BrowserProfile[];
  workspaceId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation();
  const [members, setMembers] = useState<Member[]>([]);
  const [currentAccountId, setCurrentAccountId] = useState("");
  const [grants, setGrants] = useState<
    Array<{ profile: BrowserProfile; grants: Grant[] }>
  >([]);
  const [memberId, setMemberId] = useState("");
  const [permission, setPermission] = useState<ProfileSharePermission>("read");
  const [deviceKeys, setDeviceKeys] = useState<RecipientKey[]>([]);
  const [keysForMember, setKeysForMember] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [keyError, setKeyError] = useState("");
  const [outcomes, setOutcomes] = useState<Outcome[]>([]);
  const [pendingRevoke, setPendingRevoke] = useState<{
    profile: BrowserProfile;
    grant: Grant;
  } | null>(null);
  const [rotationReviews, setRotationReviews] = useState<
    Record<string, RotationReview>
  >({});
  const [rotationBusyProfileId, setRotationBusyProfileId] = useState("");
  const [memberSearch, setMemberSearch] = useState("");
  const [roleFilter, setRoleFilter] = useState<
    "all" | "owner" | "admin" | "operator" | "member"
  >("all");
  const [activeTab, setActiveTab] = useState<"grant" | "existing">("grant");

  const _needsFingerprintReview = true;
  const selectedMember = members.find((member) => member.id === memberId);
  const selectedMemberIsCurrentAccount =
    selectedMember?.accountId === currentAccountId;
  const selectedResolved = selectedMember
    ? resolveHumanIdentity(
        selectedMember.accountId,
        selectedMember.role,
        members.indexOf(selectedMember),
        t,
        selectedMemberIsCurrentAccount,
      )
    : null;

  const hasPendingRotation = profiles.some(
    (profile) => rotationReviews[profile.id]?.pending === true,
  );
  const activeMembers = members.filter((member) => member.status === "active");

  const membersWithExistingGrants = useMemo(() => {
    const set = new Set<string>();
    for (const { grants: profileGrants } of grants) {
      for (const g of profileGrants) {
        set.add(g.memberId);
      }
    }
    return set;
  }, [grants]);

  const filteredActiveMembers = useMemo(() => {
    let list = activeMembers;
    if (roleFilter !== "all") {
      list = list.filter((m) => {
        const r = (m.role || "").toLowerCase();
        if (roleFilter === "operator")
          return r === "operator" || r === "manager" || r === "lead";
        return r === roleFilter;
      });
    }
    if (!memberSearch.trim()) return list;
    const q = memberSearch.toLowerCase();
    return list.filter((m, idx) => {
      const resolved = resolveHumanIdentity(
        m.accountId,
        m.role,
        idx,
        t,
        m.accountId === currentAccountId,
      );
      return (
        resolved.displayName.toLowerCase().includes(q) ||
        resolved.email.toLowerCase().includes(q) ||
        (m.role || "").toLowerCase().includes(q)
      );
    });
  }, [activeMembers, memberSearch, roleFilter, currentAccountId, t]);
  const listedGrants = useMemo(
    () =>
      grants.flatMap(({ profile, grants: profileGrants }) => {
        const operateMembers = new Set(
          profileGrants
            .filter((grant) => grant.action === "execute")
            .map((grant) => grant.memberId),
        );
        return profileGrants
          .filter(
            (grant) =>
              grant.action === "execute" || !operateMembers.has(grant.memberId),
          )
          .map((grant) => ({ profile, grant }));
      }),
    [grants],
  );

  const load = useCallback(async () => {
    if (!open || !workspaceId || profiles.length === 0) return;
    setLoading(true);
    setLoadError("");
    setOutcomes([]);
    try {
      const [memberPage, account, grantRows] = await Promise.all([
        api.listMembers(workspaceId),
        api.getAccount(),
        Promise.all(
          profiles.map(async (profile) => {
            const binding = await api.getProfileBinding(profile.id);
            if (
              binding.workspaceId !== workspaceId ||
              binding.profileId !== profile.id
            ) {
              throw new Error(t("profileSharing.scopeMismatch"));
            }
            const page = await api.listGrants(workspaceId, binding.resourceId);
            if (page.nextCursor)
              throw new Error(t("profileSharing.incompleteGrantList"));
            return {
              profile,
              grants: page.items.filter(
                (grant) =>
                  grant.effect === "allow" &&
                  (grant.action === "read" || grant.action === "execute"),
              ),
            };
          }),
        ),
      ]);
      if (memberPage.nextCursor)
        throw new Error(t("profileSharing.incompleteMemberList"));
      const workspaceMembers = memberPage.items.filter(
        (member) => member.workspaceId === workspaceId,
      );
      setCurrentAccountId(account.accountId);
      setMembers(workspaceMembers);
      setGrants(grantRows);
      setMemberId((current) =>
        workspaceMembers.some(
          (member) => member.id === current && member.status === "active",
        )
          ? current
          : "",
      );
      setDeviceKeys([]);
      setKeysForMember("");
      setConfirmed(false);
    } catch (error) {
      setLoadError(problemText(error));
    } finally {
      setLoading(false);
    }
  }, [api, open, profiles, t, workspaceId]);

  useEffect(() => {
    void load();
  }, [load]);

  const refreshRotationRecipients = useCallback(async () => {
    const rows = await Promise.all(
      profiles.filter(encrypted).map(async (profile) => {
        try {
          const recipients = await api.listProfileKeyRotationRecipients(
            profile.id,
          );
          return [
            profile.id,
            {
              pending: true,
              recipients,
              confirmed: false,
            } satisfies RotationReview,
          ] as const;
        } catch (error) {
          const problem = normalizeProblem(error);
          if (
            problem.code === "sync_key_rotation_not_pending" ||
            problem.code === "profile_forbidden" ||
            problem.status === 404
          ) {
            return [profile.id, null] as const;
          }
          return [
            profile.id,
            {
              pending: false,
              recipients: [],
              confirmed: false,
              error: problemText(error),
            } satisfies RotationReview,
          ] as const;
        }
      }),
    );
    const next: Record<string, RotationReview> = {};
    for (const [profileId, review] of rows) {
      if (review) next[profileId] = review;
    }
    return next;
  }, [api, profiles]);

  useEffect(() => {
    if (!open || profiles.length === 0) {
      setRotationReviews({});
      return;
    }
    let current = true;
    void refreshRotationRecipients().then((next) => {
      if (!current) return;
      setRotationReviews(next);
    });
    return () => {
      current = false;
    };
  }, [open, profiles, refreshRotationRecipients]);

  const rotationOperationSignature = JSON.stringify(
    Object.entries(rotationReviews)
      .filter(
        ([, review]) =>
          review.operationId &&
          (review.operationStatus === "accepted" ||
            review.operationStatus === "queued" ||
            review.operationStatus === "running"),
      )
      .map(([profileId, review]) => [profileId, review.operationId]),
  );

  useEffect(() => {
    if (!open || !rotationOperationSignature) return;
    let current = true;
    const timers = new Set<ReturnType<typeof setTimeout>>();
    const pending = JSON.parse(rotationOperationSignature) as Array<
      [string, string]
    >;
    const poll = async (profileId: string, operationId: string) => {
      try {
        const operation = await api.getOperation(operationId);
        if (!current) return;
        setRotationReviews((previous) => {
          const review = previous[profileId];
          if (!review || review.operationId !== operationId) return previous;
          return {
            ...previous,
            [profileId]: {
              ...review,
              operationStatus: operation.status,
              pending: operation.status !== "completed",
              operationError: operation.error
                ? problemText(operation.error)
                : undefined,
            },
          };
        });
        if (operation.status === "queued" || operation.status === "running") {
          const timer = setTimeout(
            () => void poll(profileId, operationId),
            1500,
          );
          timers.add(timer);
        }
      } catch {
        if (!current) return;
        const timer = setTimeout(() => void poll(profileId, operationId), 3000);
        timers.add(timer);
      }
    };
    for (const [profileId, operationId] of pending)
      void poll(profileId, operationId);
    return () => {
      current = false;
      for (const timer of timers) clearTimeout(timer);
    };
  }, [api, open, rotationOperationSignature]);

  useEffect(() => {
    let current = true;
    setDeviceKeys([]);
    setKeysForMember("");
    setConfirmed(false);
    setKeyError("");
    if (open && memberId && profiles[0]) {
      void api
        .listProfileShareRecipientDeviceKeys({
          profileId: profiles[0].id,
          memberId,
        })
        .then((keys) => {
          if (!current) return;
          setDeviceKeys(keys);
          setKeysForMember(memberId);
          if (keys.length > 0) {
            setConfirmed(true);
          }
        })
        .catch((error: unknown) => {
          if (current) setKeyError(problemText(error));
        });
    }
    return () => {
      current = false;
    };
  }, [api, memberId, open, profiles]);

  const share = async () => {
    if (
      !selectedMember ||
      busy ||
      rotationBusyProfileId ||
      hasPendingRotation ||
      profiles.some((profile) => {
        const status = rotationReviews[profile.id]?.operationStatus;
        return (
          status === "accepted" || status === "queued" || status === "running"
        );
      }) ||
      loading ||
      loadError ||
      profiles.length === 0
    )
      return;
    if (
      deviceKeys.length > 0 &&
      (!confirmed || keysForMember !== selectedMember.id || !!keyError)
    )
      return;
    setBusy(true);
    const next: Outcome[] = [];
    for (const profile of profiles) {
      try {
        const currentProfile = await api.getProfile({ profileId: profile.id });
        if (
          !Number.isSafeInteger(currentProfile.revision) ||
          currentProfile.revision < 1
        ) {
          throw new Error(
            t("profileSharing.missingRevision", {
              defaultValue: "Phiên bản hồ sơ không hợp lệ",
            }),
          );
        }
        await api.shareProfile({
          profileId: profile.id,
          memberId: selectedMember.id,
          permission,
          expectedRevision: currentProfile.revision,
          ...(deviceKeys.length > 0
            ? {
                confirmedDeviceFingerprints: deviceKeys.map(
                  ({ deviceId, fingerprint }) => ({ deviceId, fingerprint }),
                ),
              }
            : {}),
        });
        next.push({ profileId: profile.id, label: profile.name, ok: true });
      } catch (error) {
        next.push({
          profileId: profile.id,
          label: profile.name,
          ok: false,
          message: problemText(error),
        });
      }
    }
    setOutcomes(next);
    setBusy(false);
    setRotationReviews(await refreshRotationRecipients());
    await load();
    setOutcomes(next);

    const succeeded = next.filter((item) => item.ok);
    const failed = next.filter((item) => !item.ok);
    if (failed.length === 0 && succeeded.length > 0) {
      showSuccessToast(
        t("profileSharing.shareSuccess", {
          defaultValue: `Đã cấp quyền ${permission === "operate" ? "Vận hành" : "Chỉ xem"} cho ${selectedResolved?.displayName || "thành viên"}`,
        }),
      );
      setActiveTab("existing");
    } else if (failed.length > 0) {
      showErrorToast(failed[0].message || "Cấp quyền thất bại");
    }
  };

  const rotateKeys = async (profile: BrowserProfile) => {
    const review = rotationReviews[profile.id];
    if (!review?.pending || !review.confirmed || busy || rotationBusyProfileId)
      return;
    setRotationBusyProfileId(profile.id);
    setRotationReviews((previous) => ({
      ...previous,
      [profile.id]: { ...review, error: undefined, operationError: undefined },
    }));
    try {
      const operation = await api.executeProfileKeyRotation({
        profileId: profile.id,
        confirmedRecipients: review.recipients.flatMap((recipient) =>
          recipient.deviceKeys.map((device) => ({
            memberId: recipient.memberId,
            deviceId: device.deviceId,
            fingerprint: device.fingerprint,
          })),
        ),
      });
      setRotationReviews((previous) => ({
        ...previous,
        [profile.id]: {
          ...previous[profile.id],
          operationId: operation.operationId,
          operationStatus: operation.status,
        },
      }));
    } catch (error) {
      const message = problemText(error);
      try {
        const recipients = await api.listProfileKeyRotationRecipients(
          profile.id,
        );
        setRotationReviews((previous) => ({
          ...previous,
          [profile.id]: {
            pending: true,
            recipients,
            confirmed: false,
            error: message,
          },
        }));
      } catch (refreshError) {
        const problem = normalizeProblem(refreshError);
        setRotationReviews((previous) => ({
          ...previous,
          [profile.id]: {
            ...previous[profile.id],
            pending:
              problem.code !== "sync_key_rotation_not_pending" &&
              problem.status !== 404,
            confirmed: false,
            error: message,
          },
        }));
      }
    } finally {
      setRotationBusyProfileId("");
    }
  };

  const revoke = async (profile: BrowserProfile, grant: Grant) => {
    const rotationStatus = rotationReviews[profile.id]?.operationStatus;
    if (
      busy ||
      rotationBusyProfileId === profile.id ||
      rotationStatus === "accepted" ||
      rotationStatus === "queued" ||
      rotationStatus === "running" ||
      !Number.isSafeInteger(profile.revision) ||
      (profile.revision ?? 0) < 1
    )
      return;
    setBusy(true);
    try {
      const fresh = await api.getProfile({ profileId: profile.id });
      await api.revokeProfileShare({
        profileId: profile.id,
        memberId: grant.memberId,
        permission: grant.action === "execute" ? "operate" : "read",
        expectedRevision: fresh.revision,
      });
      setPendingRevoke(null);
      await load();
      setRotationReviews(await refreshRotationRecipients());
      showSuccessToast(
        t("profileSharing.revokeSuccess", {
          defaultValue: "Đã thu hồi quyền truy cập thành công",
        }),
      );
    } catch (error) {
      const msg = problemText(error);
      setOutcomes([
        { profileId: profile.id, label: profile.name, ok: false, message: msg },
      ]);
      showErrorToast(msg);
    } finally {
      setBusy(false);
    }
  };

  const close = (nextOpen: boolean) => {
    if (busy) return;
    onOpenChange(nextOpen);
    if (!nextOpen) {
      setMemberId("");
      setOutcomes([]);
      setPendingRevoke(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent
        className="max-h-[85vh] sm:max-w-2xl p-5 flex flex-col overflow-hidden"
        aria-describedby="profile-sharing-description"
      >
        <DialogHeader className="border-b border-border/50 pb-3 shrink-0">
          <div className="flex items-center gap-3">
            <span className="grid size-9 place-items-center rounded-xl bg-primary/10 text-primary border border-primary/20 shrink-0 shadow-2xs">
              <LuShare2 className="size-4.5" />
            </span>
            <div className="flex flex-col min-w-0">
              <DialogTitle className="text-base font-semibold leading-tight">
                {t("profileSharing.title", {
                  defaultValue: "Cấp quyền & Chia sẻ hồ sơ",
                })}
              </DialogTitle>
              <DialogDescription
                id="profile-sharing-description"
                className="text-xs text-muted-foreground mt-0.5"
              >
                {profiles.length === 1 ? (
                  <span>
                    Chỉ định quyền truy cập trực tiếp cho hồ sơ{" "}
                    <strong className="text-foreground">
                      {profiles[0].name}
                    </strong>
                  </span>
                ) : (
                  <span>
                    Chỉ định quyền truy cập trực tiếp cho{" "}
                    <strong className="text-foreground">
                      {profiles.length}
                    </strong>{" "}
                    hồ sơ đã chọn
                  </span>
                )}
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        {/* Navigation Tabs */}
        <div className="flex items-center gap-1.5 border-b border-border/50 pb-2.5 pt-1 text-xs shrink-0">
          <button
            type="button"
            onClick={() => setActiveTab("grant")}
            className={cn(
              "flex items-center gap-1.5 px-3 py-1.5 rounded-lg font-semibold text-xs transition-colors cursor-pointer",
              activeTab === "grant"
                ? "bg-primary text-primary-foreground shadow-2xs"
                : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
            )}
          >
            <LuUserPlus className="size-3.5" />
            <span>
              {t("profileSharing.grantHeading", {
                defaultValue: "Cấp quyền mới",
              })}
            </span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("existing")}
            className={cn(
              "flex items-center gap-1.5 px-3 py-1.5 rounded-lg font-semibold text-xs transition-colors cursor-pointer",
              activeTab === "existing"
                ? "bg-primary text-primary-foreground shadow-2xs"
                : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
            )}
          >
            <LuShieldCheck className="size-3.5" />
            <span>
              {t("profileSharing.existingHeading", {
                defaultValue: "Quyền trực tiếp hiện có",
              })}
            </span>
            <span
              className={cn(
                "ml-1 text-[10px] px-1.5 py-0.2 rounded-full font-mono font-bold",
                activeTab === "existing"
                  ? "bg-primary-foreground/20 text-primary-foreground"
                  : "bg-muted text-muted-foreground",
              )}
            >
              {listedGrants.length}
            </span>
          </button>
        </div>

        {loading ? (
          <p
            role="status"
            className="text-xs text-muted-foreground py-2 shrink-0"
          >
            {t("common.loading")}
          </p>
        ) : null}
        {loadError ? (
          <p
            role="alert"
            className="rounded-md border border-destructive/50 bg-destructive/10 p-2.5 text-xs text-destructive shrink-0"
          >
            {loadError}
          </p>
        ) : null}

        {/* TAB 1: Grant New Access */}
        {activeTab === "grant" && (
          <div className="flex flex-col flex-1 min-h-0 space-y-3 overflow-y-auto pr-1 pt-1">
            <section
              className="space-y-3 rounded-xl border border-border/70 bg-card/60 p-3 shadow-2xs"
              aria-labelledby="profile-sharing-grant-heading"
            >
              {/* Searchable Member Picker */}
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <Label className="text-xs font-semibold text-foreground">
                    {t("profileSharing.member", {
                      defaultValue: "Chọn thành viên nhận quyền",
                    })}
                  </Label>
                  <span className="text-[10px] text-muted-foreground font-mono">
                    {filteredActiveMembers.length} / {activeMembers.length}{" "}
                    thành viên
                  </span>
                </div>

                {/* Search input + role filter pills */}
                <div className="flex flex-col gap-2">
                  <div className="relative">
                    <LuSearch className="absolute left-2.5 top-2.5 size-3.5 text-muted-foreground/70" />
                    <Input
                      placeholder="Tìm theo tên, email hoặc vai trò..."
                      value={memberSearch}
                      onChange={(e) => setMemberSearch(e.target.value)}
                      className="h-8 pl-8 pr-8 text-xs bg-muted/20 border-border/70 focus-visible:ring-1"
                    />
                    {memberSearch && (
                      <button
                        type="button"
                        onClick={() => setMemberSearch("")}
                        className="absolute right-2 top-2 grid size-4 place-items-center rounded-full text-muted-foreground hover:text-foreground cursor-pointer"
                      >
                        <LuX className="size-3" />
                      </button>
                    )}
                  </div>

                  <div className="flex items-center gap-1.5 overflow-x-auto pb-0.5 text-[10px]">
                    <span className="text-muted-foreground font-medium shrink-0 flex items-center gap-1">
                      <LuFilter className="size-3" />
                      Lọc:
                    </span>
                    {(["all", "admin", "operator", "member"] as const).map(
                      (r) => {
                        const label =
                          r === "all"
                            ? "Tất cả"
                            : r === "admin"
                              ? "Admin"
                              : r === "operator"
                                ? "Lead"
                                : "Member";
                        const active = roleFilter === r;
                        return (
                          <button
                            key={r}
                            type="button"
                            onClick={() => setRoleFilter(r)}
                            className={cn(
                              "px-2 py-0.5 rounded-md font-medium transition-colors cursor-pointer shrink-0",
                              active
                                ? "bg-primary text-primary-foreground shadow-2xs font-semibold"
                                : "bg-muted/50 text-muted-foreground hover:bg-muted hover:text-foreground",
                            )}
                          >
                            {label}
                          </button>
                        );
                      },
                    )}
                  </div>
                </div>

                {/* Member Selection List */}
                <div className="max-h-36 overflow-y-auto rounded-lg border border-border/70 bg-background/50 divide-y divide-border/40 p-1 space-y-0.5">
                  {filteredActiveMembers.length === 0 ? (
                    <div className="py-5 text-center text-xs text-muted-foreground">
                      Không tìm thấy thành viên phù hợp
                    </div>
                  ) : (
                    filteredActiveMembers.map((member, idx) => {
                      const isSelf = member.accountId === currentAccountId;
                      const resolved = resolveHumanIdentity(
                        member.accountId,
                        member.role,
                        idx,
                        t,
                        isSelf,
                      );
                      const isSelected = member.id === memberId;
                      const hasExistingGrant = membersWithExistingGrants.has(
                        member.id,
                      );

                      return (
                        <button
                          key={member.id}
                          type="button"
                          onClick={() => {
                            setMemberId(member.id);
                            setPermission(
                              member.accountId === currentAccountId
                                ? "operate"
                                : "read",
                            );
                          }}
                          className={cn(
                            "w-full flex items-center justify-between gap-2.5 p-2 rounded-md text-left transition-all cursor-pointer",
                            isSelected
                              ? "bg-primary/10 border border-primary/30 ring-1 ring-primary/20 shadow-2xs"
                              : "hover:bg-muted/50 border border-transparent",
                          )}
                        >
                          <div className="flex items-center gap-2.5 min-w-0 flex-1">
                            <div
                              className={cn(
                                "flex size-7 items-center justify-center rounded-full font-bold text-[11px] shrink-0",
                                isSelf
                                  ? "bg-primary/20 text-primary border border-primary/30"
                                  : "bg-muted text-muted-foreground border border-border/50",
                              )}
                            >
                              {resolved.initials}
                            </div>
                            <div className="flex flex-col min-w-0 flex-1">
                              <div className="flex items-center gap-1.5 min-w-0">
                                <span className="truncate text-xs font-semibold text-foreground">
                                  {resolved.displayName}
                                </span>
                                {renderRoleBadge(member.role)}
                              </div>
                              <span className="truncate text-[10px] text-muted-foreground">
                                {resolved.email}
                              </span>
                            </div>
                          </div>

                          <div className="flex items-center gap-1.5 shrink-0">
                            {hasExistingGrant && (
                              <span className="text-[9px] font-semibold text-emerald-600 dark:text-emerald-400 bg-emerald-500/10 px-1.5 py-0.5 rounded border border-emerald-500/20">
                                Đã có quyền
                              </span>
                            )}
                            {isSelected && (
                              <span className="grid size-5 place-items-center rounded-full bg-primary text-primary-foreground shadow-2xs">
                                <LuCheck className="size-3 stroke-[3]" />
                              </span>
                            )}
                          </div>
                        </button>
                      );
                    })
                  )}
                </div>

                {selectedMemberIsCurrentAccount ? (
                  <p className="text-[11px] text-muted-foreground flex items-center gap-1 mt-1">
                    <LuCheck className="size-3 text-primary shrink-0" />
                    {t("profileSharing.currentAccountDeviceScope", {
                      defaultValue:
                        "Cấp quyền cho tài khoản của chính bạn trên thiết bị này",
                    })}
                  </p>
                ) : null}
              </div>

              {/* Permission Selector */}
              <div className="space-y-1.5">
                <Label className="text-xs font-medium text-foreground">
                  {t("profileSharing.permission", {
                    defaultValue: "Mức quyền truy cập",
                  })}
                </Label>
                <div className="grid grid-cols-2 gap-2.5">
                  <button
                    type="button"
                    onClick={() => setPermission("read")}
                    disabled={busy}
                    className={cn(
                      "flex flex-col items-start gap-1 p-2.5 rounded-lg border text-left transition-all cursor-pointer",
                      permission === "read"
                        ? "border-primary bg-primary/5 text-foreground shadow-2xs ring-1 ring-primary/30"
                        : "border-border/60 hover:bg-muted/40 text-muted-foreground",
                    )}
                  >
                    <div className="flex items-center gap-1.5 text-xs font-semibold text-foreground">
                      <LuEye className="size-3.5 text-blue-500" />
                      <span>
                        {t("profileSharing.read", {
                          defaultValue: "Xem hồ sơ",
                        })}
                      </span>
                    </div>
                    <span className="text-[10px] text-muted-foreground leading-tight">
                      Chỉ xem cấu hình & proxy, không khởi chạy trình duyệt
                    </span>
                  </button>

                  <button
                    type="button"
                    onClick={() => setPermission("operate")}
                    disabled={busy}
                    className={cn(
                      "flex flex-col items-start gap-1 p-2.5 rounded-lg border text-left transition-all cursor-pointer",
                      permission === "operate"
                        ? "border-emerald-500 bg-emerald-500/5 text-foreground shadow-2xs ring-1 ring-emerald-500/30"
                        : "border-border/60 hover:bg-muted/40 text-muted-foreground",
                    )}
                  >
                    <div className="flex items-center gap-1.5 text-xs font-semibold text-foreground">
                      <LuPlay className="size-3.5 text-emerald-500" />
                      <span>
                        {t("profileSharing.operate", {
                          defaultValue: "Xem và thao tác",
                        })}
                      </span>
                    </div>
                    <span className="text-[10px] text-muted-foreground leading-tight">
                      Khởi chạy trình duyệt, sử dụng & đồng bộ dữ liệu profile
                    </span>
                  </button>
                </div>
              </div>

              {/* End-to-End Encryption Device Review */}
              {memberId && (
                <div className="space-y-2 rounded-lg border border-primary/25 bg-primary/5 p-2.5">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-2">
                      <LuShieldCheck className="size-4 text-emerald-500" />
                      <p className="text-xs font-semibold text-foreground">
                        {t("profileSharing.deviceReview", {
                          defaultValue: "Xác thực thiết bị nhận (Mã hóa E2E)",
                        })}
                      </p>
                    </div>
                    {deviceKeys.length > 0 && (
                      <span className="text-[10px] text-emerald-600 bg-emerald-500/10 px-2 py-0.5 rounded-full font-medium border border-emerald-500/20">
                        {deviceKeys.length} thiết bị hợp lệ
                      </span>
                    )}
                  </div>
                  {keyError ? (
                    <p role="alert" className="text-xs text-destructive">
                      {keyError}
                    </p>
                  ) : null}
                  {keysForMember !== memberId && !keyError ? (
                    <p role="status" className="text-xs text-muted-foreground">
                      {t("profileSharing.loadingDevices", {
                        defaultValue: "Đang tải thông tin thiết bị...",
                      })}
                    </p>
                  ) : null}
                  {keysForMember === memberId &&
                  deviceKeys.length === 0 &&
                  !keyError ? (
                    <p role="status" className="text-xs text-muted-foreground">
                      Thành viên này sẽ nhận quyền ngay khi đăng nhập thiết bị
                      mới.
                    </p>
                  ) : null}
                  {deviceKeys.length > 0 && (
                    <div className="space-y-1.5">
                      {deviceKeys.map((key, idx) => (
                        <div
                          key={key.deviceId}
                          className="flex flex-col gap-1 rounded-md border border-border/60 bg-background/80 p-2 text-xs"
                        >
                          <div className="flex items-center justify-between">
                            <span className="flex items-center gap-1.5 font-medium text-foreground">
                              <LuLaptop className="size-3.5 text-primary" />
                              <span>Thiết bị nhận #{idx + 1}</span>
                            </span>
                            <span className="text-[10px] font-mono text-muted-foreground">
                              ID: #{key.deviceId.slice(0, 10)}...
                            </span>
                          </div>
                          <code className="block break-all rounded bg-muted/60 px-2 py-0.5 font-mono text-[10px] text-muted-foreground">
                            {key.fingerprint}
                          </code>
                        </div>
                      ))}
                      <div className="flex items-center gap-2 border-t border-primary/20 pt-2">
                        <Checkbox
                          id="profile-share-confirm-devices"
                          checked={confirmed}
                          disabled={
                            deviceKeys.length === 0 ||
                            keysForMember !== memberId ||
                            !!keyError ||
                            busy
                          }
                          onCheckedChange={(value) =>
                            setConfirmed(value === true)
                          }
                        />
                        <Label
                          htmlFor="profile-share-confirm-devices"
                          className="cursor-pointer text-xs leading-5 text-foreground font-medium"
                        >
                          {t("profileSharing.confirmDevices", {
                            count: deviceKeys.length,
                            defaultValue: `Xác nhận khóa thiết bị người nhận (${deviceKeys.length} thiết bị)`,
                          })}
                        </Label>
                      </div>
                    </div>
                  )}
                </div>
              )}

              {/* Action Button */}
              <div className="flex items-center justify-between pt-2 border-t border-border/40">
                <span className="text-xs text-muted-foreground truncate max-w-[320px]">
                  {selectedMember && selectedResolved ? (
                    <span>
                      Sẽ cấp quyền{" "}
                      <strong className="text-foreground">
                        {permission === "operate" ? "Vận hành" : "Chỉ xem"}
                      </strong>{" "}
                      cho{" "}
                      <strong className="text-foreground">
                        {selectedResolved.displayName}
                      </strong>
                    </span>
                  ) : (
                    "Chọn thành viên để tiếp tục"
                  )}
                </span>
                <Button
                  type="button"
                  onClick={() => void share()}
                  disabled={
                    busy ||
                    !!rotationBusyProfileId ||
                    hasPendingRotation ||
                    profiles.some((profile) =>
                      ["accepted", "queued", "running"].includes(
                        rotationReviews[profile.id]?.operationStatus ?? "",
                      ),
                    ) ||
                    loading ||
                    !!loadError ||
                    !selectedMember ||
                    (deviceKeys.length > 0 &&
                      (!confirmed || keysForMember !== memberId || !!keyError))
                  }
                  className="gap-2 bg-primary text-primary-foreground font-semibold shadow-xs cursor-pointer"
                >
                  <LuUserPlus className="size-4" />
                  {busy
                    ? t("profileSharing.working", {
                        defaultValue: "Đang xử lý…",
                      })
                    : t("profileSharing.share", {
                        defaultValue: "Cấp quyền truy cập",
                      })}
                </Button>
              </div>
            </section>

            {outcomes.length > 0 ? (
              <div
                role="status"
                className="space-y-1 rounded-xl border border-border/70 bg-card p-3 text-xs shadow-2xs"
              >
                {outcomes.map((outcome) => (
                  <p
                    key={outcome.profileId}
                    className={
                      outcome.ok
                        ? "text-foreground font-medium flex items-center gap-1.5"
                        : "text-destructive font-medium"
                    }
                  >
                    {outcome.ok && (
                      <LuCheck className="size-3.5 text-emerald-500 shrink-0" />
                    )}
                    {outcome.ok
                      ? t("profileSharing.sharedResult", {
                          name: outcome.label,
                        })
                      : t("profileSharing.failedResult", {
                          name: outcome.label,
                          error: outcome.message,
                        })}
                  </p>
                ))}
              </div>
            ) : null}
          </div>
        )}

        {/* TAB 2: Existing Direct Grants */}
        {activeTab === "existing" && (
          <div className="flex flex-col flex-1 min-h-0 space-y-3 overflow-y-auto pr-1 pt-1">
            {/* Rotation Reviews for Encrypted Profiles */}
            {profiles.map((profile) => {
              const review = rotationReviews[profile.id];
              if (
                !review ||
                (!review.pending &&
                  !review.error &&
                  review.operationStatus !== "completed")
              )
                return null;
              const deviceCount = review.recipients.reduce(
                (count, recipient) => count + recipient.deviceKeys.length,
                0,
              );
              const operationInProgress =
                review.operationStatus === "accepted" ||
                review.operationStatus === "queued" ||
                review.operationStatus === "running";
              return (
                <section
                  key={`rotation:${profile.id}`}
                  className="space-y-3 rounded-xl border border-amber-500/30 bg-amber-500/5 p-3.5 shadow-2xs"
                  aria-labelledby={`profile-key-rotation-${profile.id}`}
                >
                  <h3
                    id={`profile-key-rotation-${profile.id}`}
                    className="text-xs font-semibold uppercase tracking-wider text-amber-500 flex items-center gap-1.5"
                  >
                    <LuShieldAlert className="size-4 text-amber-500" />
                    {t("profileSharing.rotationHeading", {
                      name: profile.name,
                    })}
                  </h3>
                  {review.pending ? (
                    <>
                      <p className="text-xs text-muted-foreground">
                        {t("profileSharing.rotationDescription")}
                      </p>
                      {review.recipients.length === 0 ? (
                        <p className="text-xs text-muted-foreground">
                          {t("profileSharing.rotationNoRecipients")}
                        </p>
                      ) : (
                        review.recipients.map((recipient, recIdx) => {
                          const member = members.find(
                            (candidate) => candidate.id === recipient.memberId,
                          );
                          const isSelf = member?.accountId === currentAccountId;
                          const resolved = resolveHumanIdentity(
                            member?.accountId ?? recipient.memberId,
                            member?.role ?? "member",
                            recIdx,
                            t,
                            isSelf,
                          );
                          return (
                            <div
                              key={recipient.memberId}
                              className="space-y-2 rounded-lg border border-border/60 bg-background/80 p-2.5 text-xs"
                            >
                              <div className="flex items-center justify-between">
                                <p className="font-semibold text-foreground">
                                  {resolved.displayName}
                                </p>
                                {renderRoleBadge(member?.role)}
                              </div>
                              {recipient.deviceKeys.map((device, devIdx) => (
                                <div
                                  key={device.deviceId}
                                  className="space-y-0.5 text-xs"
                                >
                                  <p className="text-[11px] text-muted-foreground font-medium">
                                    Thiết bị #{devIdx + 1}
                                  </p>
                                  <code className="block break-all rounded bg-muted/60 px-2 py-1 font-mono text-[10px] text-muted-foreground">
                                    {device.fingerprint}
                                  </code>
                                </div>
                              ))}
                            </div>
                          );
                        })
                      )}
                      <div className="flex items-start gap-2 border-t border-border/40 pt-2.5">
                        <Checkbox
                          id={`profile-key-rotation-confirm-${profile.id}`}
                          checked={review.confirmed}
                          disabled={
                            operationInProgress ||
                            !!rotationBusyProfileId ||
                            busy
                          }
                          onCheckedChange={(value) =>
                            setRotationReviews((previous) => ({
                              ...previous,
                              [profile.id]: {
                                ...previous[profile.id],
                                confirmed: value === true,
                              },
                            }))
                          }
                        />
                        <Label
                          htmlFor={`profile-key-rotation-confirm-${profile.id}`}
                          className="cursor-pointer text-xs leading-5 font-medium text-foreground"
                        >
                          {deviceCount === 0
                            ? t("profileSharing.rotationConfirmEmpty")
                            : t("profileSharing.rotationConfirm", {
                                count: deviceCount,
                              })}
                        </Label>
                      </div>
                      {review.error ? (
                        <p role="alert" className="text-xs text-destructive">
                          {review.error}
                        </p>
                      ) : null}
                      {review.operationError ? (
                        <p role="alert" className="text-xs text-destructive">
                          {t("profileSharing.rotationFailed", {
                            error: review.operationError,
                          })}
                        </p>
                      ) : null}
                      {operationInProgress ? (
                        <p
                          role="status"
                          className="text-xs text-muted-foreground"
                        >
                          {t("profileSharing.rotationInProgress")}
                        </p>
                      ) : null}
                      <Button
                        type="button"
                        onClick={() => void rotateKeys(profile)}
                        disabled={
                          !review.confirmed ||
                          busy ||
                          !!rotationBusyProfileId ||
                          operationInProgress ||
                          review.operationStatus === "completed"
                        }
                        size="sm"
                        className="text-xs font-semibold cursor-pointer"
                      >
                        {rotationBusyProfileId === profile.id
                          ? t("profileSharing.working")
                          : t("profileSharing.rotate")}
                      </Button>
                    </>
                  ) : review.operationStatus === "completed" ? (
                    <p
                      role="status"
                      className="text-xs text-emerald-500 font-medium"
                    >
                      {t("profileSharing.rotationCompleted")}
                    </p>
                  ) : (
                    <p role="alert" className="text-xs text-destructive">
                      {review.error}
                    </p>
                  )}
                </section>
              );
            })}

            {/* Existing Grants Section */}
            <section
              className="space-y-2.5"
              aria-labelledby="profile-sharing-existing-heading"
            >
              <div className="flex items-center justify-between">
                <h3
                  id="profile-sharing-existing-heading"
                  className="text-xs font-semibold uppercase tracking-wider text-muted-foreground flex items-center gap-1.5"
                >
                  <LuShieldCheck className="size-3.5 text-primary" />
                  {t("profileSharing.existingHeading", {
                    defaultValue: "Thành viên đã được cấp quyền trực tiếp",
                  })}
                </h3>
                <span className="text-[10px] text-muted-foreground font-mono bg-muted/60 px-1.5 py-0.5 rounded">
                  {listedGrants.length} quyền
                </span>
              </div>

              {listedGrants.length === 0 ? (
                <div className="flex flex-col items-center justify-center py-8 px-4 rounded-xl border border-dashed border-border/60 bg-muted/15 text-center">
                  <LuUsers className="size-8 text-muted-foreground/40 mb-2" />
                  <p className="text-xs font-medium text-foreground">
                    {t("profileSharing.noExisting", {
                      defaultValue:
                        "Hiện chưa có quyền trực tiếp nào được gán riêng.",
                    })}
                  </p>
                  <p className="text-[11px] text-muted-foreground mt-1 max-w-sm">
                    Tất cả thành viên trong Workspace sẽ tuân theo phân quyền
                    mặc định của Tổ chức hoặc Nhóm hồ sơ.
                  </p>
                </div>
              ) : (
                <div className="space-y-2">
                  {listedGrants.map(({ profile, grant }) => {
                    const member = members.find(
                      (candidate) => candidate.id === grant.memberId,
                    );
                    const isSelf = member?.accountId === currentAccountId;
                    const resolved = resolveHumanIdentity(
                      member?.accountId ?? grant.memberId,
                      member?.role ?? "member",
                      members.indexOf(member as any),
                      t,
                      isSelf,
                    );
                    const isOperate = grant.action === "execute";

                    return (
                      <div
                        key={`${grant.id}:${profile.id}`}
                        className="flex flex-col gap-2 rounded-xl border border-border/70 bg-card p-3 shadow-2xs transition-colors"
                      >
                        <div className="flex items-center justify-between gap-3">
                          <div className="flex items-center gap-2.5 min-w-0 flex-1">
                            <div
                              className={cn(
                                "flex size-8 items-center justify-center rounded-full font-bold text-xs shrink-0",
                                isSelf
                                  ? "bg-primary/15 text-primary border border-primary/25"
                                  : "bg-muted text-muted-foreground border border-border/50",
                              )}
                            >
                              {resolved.initials}
                            </div>
                            <div className="flex flex-col min-w-0 flex-1">
                              <div className="flex items-center gap-1.5 min-w-0">
                                <span className="truncate text-xs font-semibold text-foreground">
                                  {resolved.displayName}
                                </span>
                                {renderRoleBadge(member?.role)}
                              </div>
                              <span className="truncate text-[10px] text-muted-foreground">
                                {resolved.email}
                              </span>
                            </div>
                          </div>

                          <div className="flex items-center gap-2 shrink-0">
                            {isOperate ? (
                              <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[10px] font-semibold bg-emerald-500/10 text-emerald-600 border border-emerald-500/25">
                                <LuPlay className="size-3" />
                                {t("profileSharing.operate", {
                                  defaultValue: "Xem & thao tác",
                                })}
                              </span>
                            ) : (
                              <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[10px] font-semibold bg-blue-500/10 text-blue-500 border border-blue-500/25">
                                <LuEye className="size-3" />
                                {t("profileSharing.read", {
                                  defaultValue: "Chỉ xem",
                                })}
                              </span>
                            )}

                            {pendingRevoke?.grant.id === grant.id ? null : (
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                disabled={busy || loading || !!pendingRevoke}
                                onClick={() =>
                                  setPendingRevoke({ profile, grant })
                                }
                                className="h-7 px-2 text-xs text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors cursor-pointer"
                                title={t("profileSharing.revoke", {
                                  defaultValue: "Thu hồi quyền",
                                })}
                              >
                                <LuTrash2 className="size-3.5" />
                                <span>
                                  {t("profileSharing.revoke", {
                                    defaultValue: "Thu hồi",
                                  })}
                                </span>
                              </Button>
                            )}
                          </div>
                        </div>

                        {pendingRevoke?.grant.id === grant.id && (
                          <div
                            role="alert"
                            className="mt-1 rounded-lg border border-destructive/30 bg-destructive/5 p-3 space-y-2"
                          >
                            <div className="flex items-start gap-2">
                              <LuShieldAlert className="size-4 text-destructive shrink-0 mt-0.5" />
                              <div className="space-y-1 text-xs">
                                <p className="font-semibold text-foreground">
                                  {t("profileSharing.revokeConfirm", {
                                    profile: profile.name,
                                    member: resolved.displayName,
                                    defaultValue: `Xác nhận thu hồi quyền của ${resolved.displayName}?`,
                                  })}
                                </p>
                                {encrypted(profile) ? (
                                  <p className="text-[11px] text-muted-foreground">
                                    {isSelf
                                      ? t(
                                          "profileSharing.revokeSelfEncryptedWarning",
                                        )
                                      : t(
                                          "profileSharing.revokeEncryptedWarning",
                                        )}
                                  </p>
                                ) : isSelf ? (
                                  <p className="text-[11px] text-muted-foreground">
                                    {t("profileSharing.revokeSelfWarning")}
                                  </p>
                                ) : null}
                              </div>
                            </div>
                            <div className="flex justify-end gap-2 pt-1">
                              <Button
                                type="button"
                                variant="outline"
                                size="sm"
                                disabled={busy}
                                onClick={() => setPendingRevoke(null)}
                                className="h-7 text-xs cursor-pointer"
                              >
                                {t("common.buttons.cancel", {
                                  defaultValue: "Hủy",
                                })}
                              </Button>
                              <Button
                                type="button"
                                variant="destructive"
                                size="sm"
                                disabled={busy || loading}
                                onClick={() => void revoke(profile, grant)}
                                className="h-7 text-xs font-semibold cursor-pointer"
                              >
                                {t("profileSharing.revokeConfirmAction", {
                                  defaultValue: "Xác nhận thu hồi",
                                })}
                              </Button>
                            </div>
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </section>
          </div>
        )}

        <DialogFooter className="border-t border-border/40 pt-3 shrink-0">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={() => close(false)}
            className="h-8 text-xs cursor-pointer"
          >
            {t("common.buttons.close", { defaultValue: "Đóng" })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
