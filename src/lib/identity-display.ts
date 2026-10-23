/** Read the identifier entered for the active or remembered desktop login. */
export function getActiveUserEmail(): string {
  if (typeof window === "undefined") return "";
  try {
    const active = window.localStorage.getItem(
      "buglogin.auth.active_identifier",
    );
    if (active?.includes("@")) return active.trim();
    const remembered = window.localStorage.getItem(
      "buglogin.auth.remember-email.v1",
    );
    if (remembered?.includes("@")) return remembered.trim();
  } catch {
    // Local storage may be disabled; identity must come from the authenticated API.
  }
  return "";
}
