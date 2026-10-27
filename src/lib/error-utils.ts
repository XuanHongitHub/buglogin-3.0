const KNOWN_ERROR_TRANSLATIONS: Record<string, string> = {
  recipient_device_fingerprint_review_required:
    "Vui lòng kiểm tra và xác nhận vân tay thiết bị người nhận trước khi chia sẻ hồ sơ mã hóa.",
  recipient_device_fingerprint_changed:
    "Khóa vân tay thiết bị người nhận đã thay đổi. Vui lòng xác nhận lại.",
  profile_key_version_stale:
    "Khóa bảo mật của hồ sơ đã thay đổi trên đám mây. Vui lòng làm mới trước khi chia sẻ.",
  profile_key_version_missing: "Hồ sơ chưa có khóa mã hóa hợp lệ.",
  sync_key_recovery_required:
    "Thiết bị này chưa khôi phục được khóa giải mã E2EE của profile. Hãy khôi phục khóa trước khi mở profile.",
  sync_key_vault_unavailable:
    "Không thể truy cập kho khóa mã hóa an toàn trên thiết bị.",
  profile_not_running: "Hồ sơ hiện không trong trạng thái hoạt động.",
  profile_access_denied: "Bạn không có quyền thao tác trên hồ sơ này.",
  workspace_access_denied:
    "Bạn không có quyền truy cập không gian làm việc này.",
  workspace_unavailable:
    "Không gian làm việc hiện không khả dụng cho tài khoản này.",
  profile_quota_exceeded:
    "Đã vượt quá hạn mức số lượng hồ sơ cho phép của Workspace.",
  profile_not_found: "Không tìm thấy hồ sơ được chỉ định.",
  member_not_found: "Không tìm thấy thành viên được chỉ định.",
  authentication_required: "Phiên làm việc đã hết hạn. Vui lòng đăng nhập lại.",
};

/**
 * Safely converts any error (Error instance, Tauri Problem object, string, or unknown)
 * into a clean, human-readable message without ever producing "[object Object]".
 */
export function formatError(error: unknown): string {
  if (!error) return "Đã xảy ra lỗi không xác định";

  if (typeof error === "string") {
    if (error === "[object Object]") return "Đã xảy ra lỗi không xác định";
    // Check if error matches any known code
    const cleanStr = error.trim();
    if (KNOWN_ERROR_TRANSLATIONS[cleanStr]) {
      return KNOWN_ERROR_TRANSLATIONS[cleanStr];
    }
    // Remove correlation IDs (e.g. " · cor_d6fca646-...")
    const sanitized = cleanStr.replace(/\s*·\s*cor_[a-zA-Z0-9_-]+/g, "").trim();
    if (sanitized.startsWith("{") && sanitized.endsWith("}")) {
      try {
        const parsed = JSON.parse(sanitized);
        return formatError(parsed);
      } catch {
        // Keep original string if parse fails
      }
    }
    return sanitized;
  }

  if (error instanceof Error) {
    if (error.message && error.message !== "[object Object]")
      return formatError(error.message);
    if ("cause" in error && error.cause !== undefined)
      return formatError(error.cause);
    return error.name || "Đã xảy ra lỗi";
  }

  if (typeof error === "object") {
    const obj = error as Record<string, unknown>;
    const code =
      typeof obj.code === "string" && obj.code.trim().length > 0
        ? obj.code.trim()
        : undefined;
    if (code && KNOWN_ERROR_TRANSLATIONS[code]) {
      return KNOWN_ERROR_TRANSLATIONS[code];
    }

    const candidate = [
      obj.detail,
      obj.message,
      obj.title,
      obj.error,
      obj.description,
    ].find(
      (value): value is string =>
        typeof value === "string" &&
        value.trim().length > 0 &&
        value !== "[object Object]",
    );
    if (candidate) {
      const cleanCandidate = candidate
        .replace(/\s*·\s*cor_[a-zA-Z0-9_-]+/g, "")
        .trim();
      return cleanCandidate;
    }
    if (obj.error && typeof obj.error === "object") {
      return formatError(obj.error);
    }
    try {
      const json = JSON.stringify(error);
      if (json && json !== "{}") return json;
    } catch {
      // ignore
    }
  }

  const str = String(error)
    .replace(/\s*·\s*cor_[a-zA-Z0-9_-]+/g, "")
    .trim();
  return str === "[object Object]" ? "Đã xảy ra lỗi không xác định" : str;
}

/**
 * Extracts the root error message from nested error strings
 * Removes redundant "Failed to..." prefixes to show only the most specific error
 */
export function extractRootError(error: unknown): string {
  if (!error) return "Unknown error";

  const errorStr = formatError(error);

  // Split by common error prefixes and take the last meaningful part
  const errorParts = errorStr.split(/Failed to [^:]+: /);
  const rootError = errorParts[errorParts.length - 1];

  // Clean up any remaining nested structure
  const cleanError = rootError.replace(/^"([^"]+)"$/, "$1");

  return cleanError || errorStr;
}

/**
 * Shows error toast with cleaned error message
 */
export function showCleanErrorToast(error: unknown, prefix?: string) {
  const rootError = extractRootError(error);
  const message = prefix ? `${prefix}: ${rootError}` : rootError;

  // Import dynamically to avoid circular dependencies
  void import("./toast-utils").then(({ showErrorToast }) => {
    showErrorToast(message);
  });
}
