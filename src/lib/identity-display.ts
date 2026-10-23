/**
 * Utility for resolving internal Canonical IDs (acct_...) into human-friendly
 * display names, real emails, and avatar initials.
 */

export function getActiveUserEmail(): string {
  if (typeof window === "undefined") return "";
  try {
    const active = window.localStorage.getItem(
      "buglogin.auth.active_identifier",
    );
    if (active?.includes("@")) return active.trim();
    const rem = window.localStorage.getItem("buglogin.auth.remember-email.v1");
    if (rem?.includes("@")) return rem.trim();
  } catch {
    // ignore
  }
  return "";
}

export interface HumanIdentity {
  displayName: string;
  email: string;
  initials: string;
  isSelf: boolean;
  shortId: string;
  roleTitle?: string;
}

export type TranslateFn = (
  key: string,
  options?: { defaultValue?: string; [key: string]: any },
) => string;

export function resolveHumanIdentity(
  accountId?: string | null,
  role?: string,
  index?: number,
  t?: TranslateFn,
  isCurrentAccount?: boolean,
): HumanIdentity {
  const activeEmail = getActiveUserEmail();
  const raw = (accountId || "").trim();
  const shortId =
    raw.length > 16 ? `#${raw.slice(0, 10)}...` : raw ? `#${raw}` : "#id";

  const ownerTitle = t
    ? t("roles.owner", { defaultValue: "Chủ sở hữu" })
    : "Workspace Owner";
  const adminTitle = t
    ? t("roles.admin", { defaultValue: "Quản trị viên" })
    : "Admin";
  const managerTitle = t
    ? t("roles.manager", { defaultValue: "Trưởng nhóm" })
    : "Team Lead";
  const memberTitle = t
    ? t("roles.member", { defaultValue: "Thành viên" })
    : "Member";

  // Current user's account explicitly flagged
  if (isCurrentAccount) {
    const cleanName = activeEmail
      ? activeEmail
          .split("@")[0]
          .replace(/[._-]/g, " ")
          .split(" ")
          .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
          .join(" ")
      : "";
    const name = cleanName
      ? `${cleanName} (Bạn)`
      : t
        ? t("profileSharing.currentAccountSelf", {
            defaultValue: "Tài khoản của tôi (Bạn)",
          })
        : "My Account (You)";
    return {
      displayName: name,
      email: activeEmail || "my-account@buglogin.local",
      initials: cleanName ? cleanName.slice(0, 2).toUpperCase() : "ME",
      isSelf: true,
      shortId,
      roleTitle: role ? role.toUpperCase() : undefined,
    };
  }

  // If the accountId is already an email
  if (raw.includes("@")) {
    const parts = raw.split("@")[0];
    const cleanName = parts
      .replace(/[._-]/g, " ")
      .split(" ")
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
      .join(" ");
    const isSelf = activeEmail
      ? raw.toLowerCase() === activeEmail.toLowerCase()
      : false;
    return {
      displayName: cleanName
        ? isSelf
          ? `${cleanName} (Bạn)`
          : cleanName
        : raw,
      email: raw,
      initials: (cleanName || raw).slice(0, 2).toUpperCase(),
      isSelf,
      shortId,
      roleTitle: role ? role.toUpperCase() : undefined,
    };
  }

  // Check if role is owner
  if (role === "owner") {
    const isSelf = !!activeEmail;
    const cleanName = activeEmail
      ? activeEmail
          .split("@")[0]
          .replace(/[._-]/g, " ")
          .split(" ")
          .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
          .join(" ")
      : "";
    const name = cleanName ? `${cleanName} (${ownerTitle})` : `${ownerTitle}`;
    return {
      displayName: name,
      email: activeEmail || "owner@workspace.local",
      initials: cleanName ? cleanName.slice(0, 2).toUpperCase() : "OW",
      isSelf,
      shortId,
      roleTitle: "OWNER",
    };
  }

  // Check if role is admin
  const memberIdx = typeof index === "number" ? index + 1 : 1;
  if (role === "admin") {
    return {
      displayName: `${adminTitle} #${memberIdx}`,
      email: `admin_${memberIdx}@workspace.local`,
      initials: `AD`,
      isSelf: false,
      shortId,
      roleTitle: "ADMIN",
    };
  }

  // Check if role is manager / operator
  if (role === "manager" || role === "operator") {
    return {
      displayName: `${managerTitle} #${memberIdx}`,
      email: `lead_${memberIdx}@workspace.local`,
      initials: `LD`,
      isSelf: false,
      shortId,
      roleTitle: "LEAD",
    };
  }

  // Member role or other
  if (raw.startsWith("acct_peer_")) {
    const peerTag = raw.replace("acct_peer_", "").toUpperCase();
    return {
      displayName: `${memberTitle} (${peerTag})`,
      email: `peer_${peerTag.toLowerCase()}@workspace.local`,
      initials: peerTag.slice(0, 2),
      isSelf: false,
      shortId,
      roleTitle: "MEMBER",
    };
  }

  return {
    displayName: `${memberTitle} #${memberIdx}`,
    email: `member_${memberIdx}@workspace.local`,
    initials: `M${memberIdx}`,
    isSelf: false,
    shortId,
    roleTitle: "MEMBER",
  };
}
