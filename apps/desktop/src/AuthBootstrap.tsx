import { listen as tauriListen } from "@tauri-apps/api/event";
import {
  AlertCircle,
  AlertTriangle,
  ArrowRight,
  Eye,
  EyeOff,
  Info,
  Lock,
  Mail,
  Moon,
  Sun,
} from "lucide-react";
import {
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { cn } from "../../../src/lib/utils.js";
import {
  DESKTOP_AUTH_STATUS_CHANGED_EVENT,
  type DesktopApi,
  normalizeProblem,
} from "./bridge.js";
import type { DesktopAuthStatus, Problem } from "./contracts.js";

const REMEMBER_EMAIL_STORAGE_KEY = "buglogin.auth.remember-email.v1";
const THEME_STORAGE_KEY = "buglogin.theme.v1";

function readRememberedEmail() {
  try {
    return window.localStorage.getItem(REMEMBER_EMAIL_STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

function formatAuthError(
  problem: Problem,
  lang: string,
): { title: string; detail: string } {
  const isVi = lang === "vi";
  if (
    problem.code === "not_found" ||
    problem.code === "invalid_credentials" ||
    problem.code === "unauthorized" ||
    problem.title?.toLowerCase().includes("not found")
  ) {
    return {
      title: isVi ? "Thông tin đăng nhập chưa đúng" : "Invalid credentials",
      detail: isVi
        ? "Tài khoản hoặc mật khẩu không chính xác. Vui lòng kiểm tra lại."
        : "The username or password you entered is incorrect. Please try again.",
    };
  }
  if (
    problem.code === "desktop_offline" ||
    problem.code?.includes("offline") ||
    problem.title?.toLowerCase().includes("unavailable")
  ) {
    return {
      title: isVi
        ? "Dịch vụ Native chưa sẵn sàng"
        : "Desktop service unavailable",
      detail: isVi
        ? "Dịch vụ lõi BugLogin đang khởi động hoặc mất kết nối. Vui lòng bấm thử lại."
        : "BugLogin core engine is starting up. Please click retry in a moment.",
    };
  }
  return {
    title: problem.title || (isVi ? "Lỗi đăng nhập" : "Sign in error"),
    detail:
      problem.detail ||
      (isVi
        ? "Đã xảy ra sự cố trong quá trình xác thực tài khoản."
        : "An error occurred during authentication."),
  };
}

export function AuthBootstrap({
  api,
  children,
}: {
  api: DesktopApi;
  children: ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<DesktopAuthStatus>();
  const [problem, setProblem] = useState<Problem>();
  const [identifier, setIdentifier] = useState(readRememberedEmail);
  const [password, setPassword] = useState("");
  const [rememberEmail, setRememberEmail] = useState(true);
  const [showPassword, setShowPassword] = useState(false);
  const [capsLockOn, setCapsLockOn] = useState(false);
  const [showContactAdmin, setShowContactAdmin] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loadingStatus, setLoadingStatus] = useState(true);
  const callbackInFlight = useRef(false);

  const [darkMode, setDarkMode] = useState(() => {
    try {
      const saved = window.localStorage.getItem(THEME_STORAGE_KEY);
      if (saved !== null) {
        return saved === "dark";
      }
      return (
        document.documentElement.classList.contains("dark") ||
        window.matchMedia("(prefers-color-scheme: dark)").matches
      );
    } catch {
      return true;
    }
  });

  useEffect(() => {
    document.documentElement.classList.toggle("dark", darkMode);
    try {
      window.localStorage.setItem(
        THEME_STORAGE_KEY,
        darkMode ? "dark" : "light",
      );
    } catch {
      // LocalStorage fallback
    }
  }, [darkMode]);

  const load = useCallback(async () => {
    setLoadingStatus(true);
    try {
      setProblem(undefined);
      setStatus(await api.authStatus());
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setLoadingStatus(false);
    }
  }, [api]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    window.addEventListener(DESKTOP_AUTH_STATUS_CHANGED_EVENT, load);
    return () =>
      window.removeEventListener(DESKTOP_AUTH_STATUS_CHANGED_EVENT, load);
  }, [load]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const consumeCallback = async (rawUrl: string) => {
      if (disposed || callbackInFlight.current) return;
      let callback: URL;
      try {
        callback = new URL(rawUrl);
      } catch {
        return;
      }
      if (
        callback.protocol !== "buglogin:" ||
        callback.hostname !== "sso" ||
        callback.pathname !== "/callback" ||
        callback.hash ||
        callback.username ||
        callback.password
      ) {
        return;
      }
      const states = callback.searchParams.getAll("state");
      const tokens = callback.searchParams.getAll("token");
      if (states.length !== 1 || tokens.length !== 1) return;
      const state = states[0];
      const token = tokens[0];
      if (!state || !token) return;
      callbackInFlight.current = true;
      setBusy(true);
      setProblem(undefined);
      try {
        setStatus(await api.completeBugLabSso(state, token));
      } catch (error) {
        setProblem(normalizeProblem(error));
      } finally {
        callbackInFlight.current = false;
        setBusy(false);
      }
    };
    void (async () => {
      try {
        const stop = await tauriListen<string>(
          "buglogin://deep-link",
          (event) => {
            void consumeCallback(event.payload);
          },
        );
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
        const initialUrls = await api.currentDeepLinks();
        for (const url of initialUrls) {
          await consumeCallback(url);
        }
      } catch {
        // Deep links are optional when this component is rendered outside Tauri.
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [api]);

  const startBugLabSso = async () => {
    if (busy) return;
    setBusy(true);
    setProblem(undefined);
    try {
      await api.startBugLabSso();
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setBusy(false);
    }
  };

  if (
    status?.state === "authenticated" ||
    status?.state === "auth_not_applicable"
  ) {
    return children;
  }

  if (loadingStatus) {
    return (
      <main
        className="flex min-h-screen w-full items-center justify-center bg-zinc-50 text-zinc-900 dark:bg-[#09090b] dark:text-white"
        aria-busy="true"
      >
        <div className="flex flex-col items-center gap-4">
          <div className="flex size-14 items-center justify-center rounded-2xl border border-zinc-200/80 bg-white p-2.5 shadow-md dark:border-zinc-800 dark:bg-zinc-900/90">
            <img
              src="/logo.png"
              alt="BugLogin"
              className="size-full object-contain animate-pulse"
            />
          </div>
          <div className="flex flex-col items-center gap-2">
            <span className="text-xs font-medium tracking-tight text-zinc-600 dark:text-zinc-400">
              {i18n.language === "vi"
                ? "Đang khởi động BugLogin..."
                : "Starting BugLogin..."}
            </span>
            <div className="h-1 w-24 overflow-hidden rounded-full bg-zinc-200 dark:bg-zinc-800">
              <div className="h-full w-full rounded-full bg-zinc-900 dark:bg-white opacity-40 animate-pulse" />
            </div>
          </div>
        </div>
      </main>
    );
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || !identifier.trim() || !password) return;

    setBusy(true);
    setProblem(undefined);
    try {
      const nextStatus = await api.login(identifier.trim(), password);
      setPassword("");
      setStatus(nextStatus);
      try {
        window.localStorage.setItem(
          "buglogin.auth.active_identifier",
          identifier.trim(),
        );
        if (rememberEmail) {
          window.localStorage.setItem(
            REMEMBER_EMAIL_STORAGE_KEY,
            identifier.trim(),
          );
        } else {
          window.localStorage.removeItem(REMEMBER_EMAIL_STORAGE_KEY);
        }
      } catch {
        // LocalStorage fallback
      }
    } catch (error) {
      setProblem(normalizeProblem(error));
    } finally {
      setBusy(false);
    }
  };

  const handlePasswordKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    setCapsLockOn(e.getModifierState("CapsLock"));
  };

  const handlePasswordKeyUp = (e: KeyboardEvent<HTMLInputElement>) => {
    setCapsLockOn(e.getModifierState("CapsLock"));
  };

  const formattedError = problem
    ? formatAuthError(problem, i18n.language)
    : null;

  return (
    <main className="relative flex min-h-screen w-full items-center justify-center p-4 md:p-6 bg-zinc-100 dark:bg-[#09090b] text-zinc-900 dark:text-white transition-colors duration-200 selection:bg-zinc-900 selection:text-white dark:selection:bg-white dark:selection:text-zinc-900">
      {/* 2-Column Split Card */}
      <section className="relative z-10 w-full max-w-[860px]">
        <div className="grid overflow-hidden rounded-2xl border border-zinc-200/80 bg-white shadow-xl dark:border-zinc-800 dark:bg-[#121215] dark:shadow-2xl dark:shadow-black/80 md:grid-cols-[1.1fr_0.9fr] md:min-h-[550px]">
          {/* Left Column: Form */}
          <div className="flex flex-col justify-between p-6 sm:p-8 md:p-10">
            <div>
              {/* Header: Brand & Controls */}
              <div className="flex items-center justify-between pb-6 border-b border-zinc-100 dark:border-zinc-800/70">
                <div className="flex items-center gap-3">
                  <div className="flex size-10 items-center justify-center rounded-xl border border-zinc-200 bg-zinc-50 p-2 shadow-xs dark:border-zinc-800 dark:bg-zinc-900">
                    <img
                      src="/logo.png"
                      alt="BugLogin"
                      className="size-full object-contain"
                    />
                  </div>
                  <div>
                    <h1 className="text-base font-semibold tracking-tight text-zinc-900 dark:text-white leading-tight">
                      BugLogin
                    </h1>
                    <p className="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
                      {t("desktopAuth.subtitle", {
                        defaultValue: "Đăng nhập để tiếp tục",
                      })}
                    </p>
                  </div>
                </div>

                {/* Top Controls: Theme + Language */}
                <div className="flex items-center gap-1.5">
                  {/* Theme Mode Toggle (Light / Dark) */}
                  <button
                    type="button"
                    aria-label={
                      darkMode
                        ? t("desktopAuth.lightMode", {
                            defaultValue: "Chuyển sang giao diện sáng",
                          })
                        : t("desktopAuth.darkMode", {
                            defaultValue: "Chuyển sang giao diện tối",
                          })
                    }
                    onClick={() => setDarkMode((prev) => !prev)}
                    className="flex size-7 items-center justify-center rounded-lg border border-zinc-200 bg-zinc-50 text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-white cursor-pointer"
                  >
                    {darkMode ? (
                      <Sun className="size-3.5 text-zinc-400 hover:text-white transition-colors" />
                    ) : (
                      <Moon className="size-3.5 text-zinc-600 hover:text-zinc-900 transition-colors" />
                    )}
                  </button>

                  {/* Language Switcher */}
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() =>
                      void i18n.changeLanguage(
                        i18n.language === "vi" ? "en" : "vi",
                      )
                    }
                    className="h-7 rounded-lg border border-zinc-200 bg-zinc-50 px-2.5 text-[11px] font-medium text-zinc-700 transition-colors hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-white cursor-pointer"
                  >
                    {i18n.language === "vi" ? "VI" : "EN"}
                  </button>
                </div>
              </div>

              {/* Login Form */}
              <form
                className="space-y-4 pt-6"
                onSubmit={(event) => void submit(event)}
              >
                {/* Email / Username Field */}
                <div className="space-y-1.5">
                  <label
                    htmlFor="desktop-login-identifier"
                    className="text-xs font-medium text-zinc-700 dark:text-zinc-300 leading-none"
                  >
                    {t("desktopAuth.identifierLabel", {
                      defaultValue: "Email hoặc Tên tài khoản",
                    })}
                  </label>
                  <div className="relative flex items-center">
                    <div className="pointer-events-none absolute inset-y-0 left-0 flex items-center pl-3.5 text-zinc-400 dark:text-zinc-500">
                      <Mail className="size-4" />
                    </div>
                    <input
                      id="desktop-login-identifier"
                      name="username"
                      type="text"
                      autoComplete="username"
                      placeholder="name@company.com"
                      value={identifier}
                      onChange={(event) => {
                        setIdentifier(event.target.value);
                      }}
                      disabled={busy || loadingStatus}
                      required
                      className="h-10 w-full rounded-xl border border-zinc-200 bg-zinc-50/70 pl-10 pr-3.5 text-sm text-zinc-900 placeholder:text-zinc-400 outline-none transition-colors focus:border-zinc-900 focus:ring-1 focus:ring-zinc-900/20 disabled:opacity-50 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-100 dark:placeholder:text-zinc-600 dark:focus:border-zinc-400 dark:focus:ring-white/20"
                    />
                  </div>
                </div>

                {/* Password Field */}
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between gap-2">
                    <label
                      htmlFor="desktop-login-password"
                      className="text-xs font-medium text-zinc-700 dark:text-zinc-300 leading-none"
                    >
                      {t("desktopAuth.passwordLabel", {
                        defaultValue: "Mật khẩu",
                      })}
                    </label>
                    <button
                      type="button"
                      onClick={() => setShowContactAdmin((v) => !v)}
                      className="text-[11px] text-zinc-500 hover:text-zinc-800 transition-colors dark:text-zinc-400 dark:hover:text-zinc-200 cursor-pointer"
                    >
                      {t("desktopAuth.forgotPassword", {
                        defaultValue: "Quên mật khẩu?",
                      })}
                    </button>
                  </div>

                  <div className="relative flex items-center">
                    <div className="pointer-events-none absolute inset-y-0 left-0 flex items-center pl-3.5 text-zinc-400 dark:text-zinc-500">
                      <Lock className="size-4" />
                    </div>
                    <input
                      id="desktop-login-password"
                      name="password"
                      type={showPassword ? "text" : "password"}
                      autoComplete="current-password"
                      placeholder="••••••••••••"
                      value={password}
                      onChange={(event) => {
                        setPassword(event.target.value);
                      }}
                      onKeyDown={handlePasswordKeyDown}
                      onKeyUp={handlePasswordKeyUp}
                      disabled={busy || loadingStatus}
                      required
                      className="h-10 w-full rounded-xl border border-zinc-200 bg-zinc-50/70 pl-10 pr-10 text-sm text-zinc-900 placeholder:text-zinc-400 outline-none transition-colors focus:border-zinc-900 focus:ring-1 focus:ring-zinc-900/20 disabled:opacity-50 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-100 dark:placeholder:text-zinc-600 dark:focus:border-zinc-400 dark:focus:ring-white/20"
                    />
                    <button
                      type="button"
                      className="absolute inset-y-0 right-0 flex items-center pr-3 text-zinc-400 hover:text-zinc-700 transition-colors dark:text-zinc-500 dark:hover:text-zinc-300 cursor-pointer"
                      onClick={() => setShowPassword((v) => !v)}
                      tabIndex={-1}
                      aria-label={
                        showPassword
                          ? t("desktopAuth.hidePassword")
                          : t("desktopAuth.showPassword")
                      }
                    >
                      {showPassword ? (
                        <EyeOff className="size-4" />
                      ) : (
                        <Eye className="size-4" />
                      )}
                    </button>
                  </div>

                  {/* Caps Lock Alert */}
                  {capsLockOn && (
                    <div
                      className="flex items-center gap-1.5 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-1.5 text-[11px] text-amber-600 dark:text-amber-400 font-medium"
                      role="status"
                    >
                      <AlertTriangle className="size-3.5 shrink-0 text-amber-500 dark:text-amber-400" />
                      <span>
                        {t("desktopAuth.capsLockOn", {
                          defaultValue: "Phím Caps Lock đang bật",
                        })}
                      </span>
                    </div>
                  )}

                  {/* Contact Admin Hint Alert */}
                  {showContactAdmin && (
                    <div className="flex items-start gap-2.5 rounded-xl border border-zinc-200 bg-zinc-50 p-3 text-xs text-zinc-600 dark:border-zinc-800 dark:bg-zinc-900/80 dark:text-zinc-300">
                      <Info className="size-4 text-zinc-500 dark:text-zinc-400 shrink-0 mt-0.5" />
                      <p className="text-[11px] leading-relaxed text-zinc-600 dark:text-zinc-400">
                        {t("desktopAuth.contactAdmin", {
                          defaultValue:
                            "Tài khoản BugLogin do Quản trị viên Tổ chức quản lý. Vui lòng liên hệ Admin để được cấp lại mật khẩu.",
                        })}
                      </p>
                    </div>
                  )}
                </div>

                {/* Remember Me */}
                <label
                  htmlFor="desktop-remember-email"
                  className="group flex items-center gap-2.5 text-xs text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-200 cursor-pointer select-none pt-0.5 transition-colors"
                >
                  <div className="relative flex items-center justify-center">
                    <input
                      id="desktop-remember-email"
                      type="checkbox"
                      checked={rememberEmail}
                      onChange={(e) => setRememberEmail(e.target.checked)}
                      disabled={busy}
                      className="peer sr-only"
                    />
                    <div className="flex size-[17px] items-center justify-center rounded-[5px] border border-zinc-300 bg-white transition-all duration-150 peer-focus-visible:ring-2 peer-focus-visible:ring-zinc-400 peer-checked:border-zinc-900 peer-checked:bg-zinc-900 peer-checked:text-white dark:border-zinc-700 dark:bg-zinc-900/60 dark:peer-checked:border-white dark:peer-checked:bg-white dark:peer-checked:text-zinc-950 dark:peer-focus-visible:ring-zinc-600 group-hover:border-zinc-400 dark:group-hover:border-zinc-500 shadow-xs">
                      {rememberEmail && (
                        <svg
                          aria-hidden="true"
                          className="size-2.5 stroke-[2.75] animate-in zoom-in-75 duration-150"
                          viewBox="0 0 12 12"
                          fill="none"
                          stroke="currentColor"
                          strokeLinecap="round"
                          strokeLinejoin="round"
                        >
                          <polyline points="2.5 6 4.75 8.25 9.5 3.5" />
                        </svg>
                      )}
                    </div>
                  </div>
                  <span>
                    {t("desktopAuth.rememberEmail", {
                      defaultValue: "Ghi nhớ tài khoản trên máy này",
                    })}
                  </span>
                </label>

                {/* Refined Error Notification Alert */}
                <div
                  className={cn(
                    "grid transition-all duration-300 ease-in-out",
                    formattedError
                      ? "grid-rows-[1fr] opacity-100"
                      : "grid-rows-[0fr] opacity-0 pointer-events-none",
                    busy && "opacity-60",
                  )}
                >
                  <div className="overflow-hidden">
                    {formattedError && (
                      <div
                        className="flex items-start gap-3 rounded-xl border border-red-500/30 bg-red-50 p-3 text-xs text-red-700 dark:bg-red-950/20 dark:text-red-200"
                        role="alert"
                      >
                        <AlertCircle className="size-4 shrink-0 text-red-500 dark:text-red-400 mt-0.5" />
                        <div className="flex-1 space-y-1">
                          <div className="font-semibold text-red-800 dark:text-red-300 leading-tight">
                            {formattedError.title}
                          </div>
                          <p className="text-[11px] text-red-700/80 dark:text-red-300/80 leading-relaxed">
                            {formattedError.detail}
                          </p>
                          {status === undefined && !loadingStatus && (
                            <button
                              type="button"
                              className="mt-2 inline-flex items-center gap-1.5 rounded-md border border-red-500/30 bg-red-100 px-2.5 py-1 text-[11px] font-medium text-red-800 hover:bg-red-200 dark:bg-red-500/10 dark:text-red-300 dark:hover:bg-red-500/20 transition-colors cursor-pointer"
                              onClick={() => void load()}
                            >
                              {t("common.buttons.retry", {
                                defaultValue: "Thử lại kết nối",
                              })}
                            </button>
                          )}
                        </div>
                      </div>
                    )}
                  </div>
                </div>

                {/* Submit Button (White/Black Minimalist Luxury) */}
                <button
                  type="submit"
                  className="flex h-10 w-full items-center justify-center gap-2 rounded-xl bg-zinc-900 text-xs font-semibold text-white shadow-xs transition-all hover:bg-zinc-800 active:bg-zinc-950 disabled:opacity-50 disabled:pointer-events-none cursor-pointer dark:bg-white dark:text-zinc-900 dark:hover:bg-zinc-100 dark:active:bg-zinc-200"
                  disabled={
                    busy || loadingStatus || !identifier.trim() || !password
                  }
                >
                  {busy ? (
                    <div className="flex items-center gap-2">
                      <span className="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
                      <span>
                        {t("common.buttons.loading", {
                          defaultValue: "Đang đăng nhập...",
                        })}
                      </span>
                    </div>
                  ) : (
                    <div className="flex items-center justify-center gap-2">
                      <span>
                        {t("desktopAuth.submit", { defaultValue: "Đăng nhập" })}
                      </span>
                      <ArrowRight className="size-3.5" />
                    </div>
                  )}
                </button>
              </form>
              <div className="flex items-center gap-3 py-4" aria-hidden="true">
                <div className="h-px flex-1 bg-zinc-200 dark:bg-zinc-800" />
                <span className="text-[11px] text-zinc-500 dark:text-zinc-500">
                  {i18n.language === "vi" ? "hoặc" : "or"}
                </span>
                <div className="h-px flex-1 bg-zinc-200 dark:bg-zinc-800" />
              </div>
              <button
                type="button"
                onClick={() => void startBugLabSso()}
                disabled={busy || loadingStatus}
                className="flex h-10 w-full items-center justify-center gap-2 rounded-xl border border-zinc-200 bg-white text-xs font-semibold text-zinc-800 transition-colors hover:bg-zinc-50 disabled:pointer-events-none disabled:opacity-50 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-100 dark:hover:bg-zinc-800"
              >
                {busy ? (
                  <span className="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
                ) : null}
                {i18n.language === "vi"
                  ? "Tiếp tục với BugLab ID"
                  : "Continue with BugLab ID"}
              </button>
            </div>

            {/* Clean Footer */}
            <p className="text-[11px] text-zinc-400 dark:text-zinc-600 text-center pt-6">
              © 2026 BugLogin • All rights reserved
            </p>
          </div>

          {/* Right Column: Visual Artwork (Hidden on mobile, prominent on desktop) */}
          <div className="relative hidden md:block overflow-hidden border-l border-zinc-200 dark:border-zinc-800 bg-zinc-950">
            <img
              src="/img_buglogin/browser-multi-account-platform.webp"
              alt="BugLogin Workspace"
              className="size-full object-cover object-center transition-transform duration-700 hover:scale-105"
            />
            {/* Subtle bottom gradient shadow */}
            <div className="pointer-events-none absolute inset-0 bg-gradient-to-t from-black/60 via-transparent to-transparent" />
          </div>
        </div>
      </section>
    </main>
  );
}
