import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCheck,
  LuEye,
  LuFilter,
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
import {
  showErrorToast,
  showSuccessToast,
} from "../../../src/lib/toast-utils.js";
import { cn } from "../../../src/lib/utils.js";
import type { BrowserProfile } from "../../../src/types.js";
import { type DesktopApi } from "./bridge.js";
import type { ProfileSharePermission } from "./contracts.js";

type Grant = Awaited<ReturnType<DesktopApi["listGrants"]>>["items"][number];
type Member = Awaited<ReturnType<DesktopApi["listMembers"]>>["items"][number];
function displayMember(member?: Member | null) {
  const username = member?.identity?.username;
  return {
    displayName: username ?? "Danh tính chưa được cấp quyền xem",
    email: member?.identity?.email ?? "—",
    initials: username?.slice(0, 2).toUpperCase() ?? "?",
  };
}

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
  if (r === "lead") {
    return (
      <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-emerald-500/10 text-emerald-400 border-emerald-500/30 shrink-0">
        LEAD
      </span>
    );
  }
  if (r === "operator") {
    return (
      <span className="text-[8px] px-1 py-0 h-3.5 inline-flex items-center font-bold tracking-tight border uppercase rounded bg-muted/80 text-muted-foreground/90 border-border/60 shrink-0">
        OPERATOR
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
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [outcomes, setOutcomes] = useState<Outcome[]>([]);
  const [pendingRevoke, setPendingRevoke] = useState<{
    profile: BrowserProfile;
    grant: Grant;
  } | null>(null);
  const [memberSearch, setMemberSearch] = useState("");
  const [roleFilter, setRoleFilter] = useState<
    "all" | "owner" | "admin" | "lead" | "operator" | "member"
  >("all");
  const [activeTab, setActiveTab] = useState<"grant" | "existing">("grant");

  const selectedMember = members.find((member) => member.id === memberId);
  const selectedMemberIsCurrentAccount =
    selectedMember?.accountId === currentAccountId;
  const selectedResolved = selectedMember
    ? displayMember(selectedMember)
    : null;

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
        if (roleFilter === "operator") return r === "operator";
        if (roleFilter === "lead") return r === "lead";
        return r === roleFilter;
      });
    }
    if (!memberSearch.trim()) return list;
    const q = memberSearch.toLowerCase();
    return list.filter((member) => {
      return [
        member.identity?.username,
        member.identity?.email,
        member.role,
        member.accountId,
      ]
        .filter(Boolean)
        .some((value) => value?.toLowerCase().includes(q));
    });
  }, [activeMembers, memberSearch, roleFilter]);
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
    } catch (error) {
      setLoadError(problemText(error));
    } finally {
      setLoading(false);
    }
  }, [api, open, profiles, t, workspaceId]);

  useEffect(() => {
    void load();
  }, [load]);

  const share = async () => {
    if (
      !selectedMember ||
      busy ||
      loading ||
      loadError ||
      profiles.length === 0
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

  const revoke = async (profile: BrowserProfile, grant: Grant) => {
    if (
      busy ||
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
        className="flex max-h-[min(88vh,56rem)] max-w-[min(64rem,calc(100%-2rem))] flex-col overflow-hidden p-4 sm:p-5"
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
              className="grid min-w-0 gap-4 rounded-xl border border-border/70 bg-card/60 p-3 shadow-2xs lg:grid-cols-[minmax(0,1.1fr)_minmax(19rem,0.9fr)] lg:gap-5 lg:p-4"
              aria-labelledby="profile-sharing-grant-heading"
            >
              {/* Searchable Member Picker */}
              <div className="min-w-0 space-y-2 lg:row-span-2">
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
                    {(
                      [
                        "all",
                        "owner",
                        "admin",
                        "lead",
                        "operator",
                        "member",
                      ] as const
                    ).map((r) => {
                      const labels = {
                        all: "Tất cả",
                        owner: "Owner",
                        admin: "Admin",
                        lead: "Lead",
                        operator: "Operator",
                        member: "Member",
                      };
                      const label = labels[r];
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
                    })}
                  </div>
                </div>

                {/* Member Selection List */}
                <div className="max-h-[min(48vh,28rem)] min-h-32 overflow-y-auto rounded-lg border border-border/70 bg-background/50 divide-y divide-border/40 p-1 space-y-0.5">
                  {filteredActiveMembers.length === 0 ? (
                    <div className="py-5 text-center text-xs text-muted-foreground">
                      Không tìm thấy thành viên phù hợp
                    </div>
                  ) : (
                    filteredActiveMembers.map((member) => {
                      const isSelf = member.accountId === currentAccountId;
                      const resolved = displayMember(member);
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
              <div className="min-w-0 space-y-1.5">
                <Label className="text-xs font-medium text-foreground">
                  {t("profileSharing.permission", {
                    defaultValue: "Mức quyền truy cập",
                  })}
                </Label>
                <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
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

              {/* Action Button */}
              <div className="flex min-w-0 flex-col gap-3 border-t border-border/40 pt-3 sm:flex-row sm:items-center sm:justify-between">
                <span className="min-w-0 text-xs text-muted-foreground sm:line-clamp-2">
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
                  disabled={busy || loading || !!loadError || !selectedMember}
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
                    const resolved = displayMember(member);
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
