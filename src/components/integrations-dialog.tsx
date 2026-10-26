"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuActivity,
  LuBookOpen,
  LuBot,
  LuCheck,
  LuCode,
  LuCopy,
  LuCpu,
  LuEye,
  LuEyeOff,
  LuKey,
  LuLayers,
  LuPlug,
  LuRefreshCw,
  LuServer,
  LuShieldCheck,
  LuSparkles,
  LuTrash2,
  LuTriangleAlert,
  LuZap,
} from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import type { DesktopIntegrationStatus } from "../../apps/desktop/src/contracts";
import type { ApiToken } from "../../frontend/src/generated/command-registry";

interface IntegrationsDialogProps {
  isOpen: boolean;
  onClose: () => void;
  subPage?: boolean;
  initialTab?: "api" | "remote";
}

const desktopApi = createTauriDesktopApi();

const MCP_STORAGE_KEY = "buglogin.mcp.fixed_token.v1";

interface StoredFixedMcpToken {
  token: string;
  tokenId?: string;
  createdAt: number;
  expiresAt: number;
}

function getStoredMcpToken(): StoredFixedMcpToken | null {
  if (typeof window === "undefined" || !window.localStorage) return null;
  try {
    const raw = window.localStorage.getItem(MCP_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as StoredFixedMcpToken;
    if (
      parsed &&
      typeof parsed.token === "string" &&
      parsed.token.startsWith("blapi_") &&
      parsed.expiresAt > Math.floor(Date.now() / 1000)
    ) {
      return parsed;
    }
    window.localStorage.removeItem(MCP_STORAGE_KEY);
    return null;
  } catch {
    return null;
  }
}

function saveStoredMcpToken(data: StoredFixedMcpToken) {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.setItem(MCP_STORAGE_KEY, JSON.stringify(data));
  } catch {
    // ignore
  }
}

function removeStoredMcpToken() {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.removeItem(MCP_STORAGE_KEY);
  } catch {
    // ignore
  }
}

type TabType = "api" | "mcp" | "endpoints";
type CodeLang = "curl" | "node" | "python";
type McpClient = "claude" | "cursor" | "antigravity";

export function IntegrationsDialog({
  isOpen,
  onClose,
  subPage,
  initialTab = "api",
}: IntegrationsDialogProps) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<DesktopIntegrationStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [activeTab, setActiveTab] = useState<TabType>(
    initialTab === "remote" ? "mcp" : "api",
  );
  const [codeLang, setCodeLang] = useState<CodeLang>("curl");
  const [mcpClient, setMcpClient] = useState<McpClient>("cursor");
  const [copiedKey, setCopiedKey] = useState<string | null>(null);
  const [apiTokens, setApiTokens] = useState<ApiToken[]>([]);
  const [fixedMcpToken, setFixedMcpToken] = useState<string | null>(() => {
    const stored = getStoredMcpToken();
    return stored?.token ?? null;
  });
  const [showMcpKey, setShowMcpKey] = useState(false);
  const [isManagingMcpToken, setIsManagingMcpToken] = useState(false);

  // Realtime Ping test state
  const [isPinging, setIsPinging] = useState(false);
  const [pingResult, setPingResult] = useState<{
    latencyMs: number;
    statusText: string;
    timestamp: number;
  } | null>(null);

  useEffect(() => {
    setActiveTab(initialTab === "remote" ? "mcp" : "api");
  }, [initialTab]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await desktopApi.getIntegrationStatus();
      setStatus(res);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (isOpen) void refresh();
  }, [isOpen, refresh]);

  useEffect(() => {
    if (!isOpen) return;
    void desktopApi
      .listApiTokens()
      .then((page) => setApiTokens(page.tokens))
      .catch(() => {
        setApiTokens([]);
      });
  }, [isOpen]);

  const endpoint = status?.apiEndpoint || "http://127.0.0.1:12345";
  const mcpUrl = status?.mcpEndpoint || `${endpoint}/v1/mcp`;
  const isApiConnected = status?.apiReachable ?? true;
  const mcpEnabled = status?.mcpEnabled === true;
  const activeMcpTokens = useMemo(
    () =>
      apiTokens.filter(
        (token) =>
          token.name === "BugLogin MCP" &&
          token.revokedAt == null &&
          token.expiresAt > Math.floor(Date.now() / 1000),
      ),
    [apiTokens],
  );

  const primaryMcpRecord = useMemo(() => {
    if (!activeMcpTokens.length) return null;
    const stored = getStoredMcpToken();
    if (stored?.tokenId) {
      const match = activeMcpTokens.find((t) => t.id === stored.tokenId);
      if (match) return match;
    }
    return activeMcpTokens[0] ?? null;
  }, [activeMcpTokens]);

  useEffect(() => {
    const stored = getStoredMcpToken();
    if (!stored) {
      setFixedMcpToken(null);
      return;
    }
    if (apiTokens.length > 0 && stored.tokenId) {
      const serverToken = apiTokens.find((t) => t.id === stored.tokenId);
      if (serverToken && serverToken.revokedAt != null) {
        removeStoredMcpToken();
        setFixedMcpToken(null);
        return;
      }
    }
    setFixedMcpToken(stored.token);
  }, [apiTokens]);

  const handleCopy = async (
    text: string,
    key: string,
    label = "Đã sao chép vào bộ nhớ tạm",
  ) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedKey(key);
      showSuccessToast(label);
      setTimeout(() => setCopiedKey(null), 2000);
    } catch {
      showErrorToast("Không thể sao chép");
    }
  };

  const handleCreateOrRegenerateMcpToken = async () => {
    setIsManagingMcpToken(true);
    try {
      // Clean up any existing duplicate / old MCP tokens on backend so only 1 fixed key exists
      for (const oldToken of activeMcpTokens) {
        try {
          await desktopApi.revokeApiToken(oldToken.id);
        } catch {
          // ignore
        }
      }

      // 1 year (365 days) fixed key
      const nowSec = Math.floor(Date.now() / 1000);
      const expiresAt = nowSec + 365 * 24 * 60 * 60;
      const created = await desktopApi.createApiToken({
        name: "BugLogin MCP",
        scopes: [
          "profile.read:read",
          "profile.create:create",
          "profile.update:update",
          "profile.delete:delete",
          "profile.launch:execute",
          "profile.stop:execute",
          "CAP-NET-001:read",
          "sync.manifest:read",
          "sync.manifest:update",
        ],
        expiresAt,
      });

      saveStoredMcpToken({
        token: created.token,
        tokenId: created.tokenRecord.id,
        createdAt: nowSec,
        expiresAt,
      });

      setFixedMcpToken(created.token);
      setApiTokens((current) => [
        created.tokenRecord,
        ...current.filter((t) => t.name !== "BugLogin MCP"),
      ]);

      showSuccessToast(
        "Đã kích hoạt Key MCP cố định! Key đã được lưu an toàn trên máy.",
      );
    } catch (cause) {
      showErrorToast(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsManagingMcpToken(false);
    }
  };

  const handleRevokeFixedMcpToken = async () => {
    setIsManagingMcpToken(true);
    try {
      const stored = getStoredMcpToken();
      for (const oldToken of activeMcpTokens) {
        try {
          await desktopApi.revokeApiToken(oldToken.id);
        } catch {
          // continue
        }
      }
      if (stored?.tokenId) {
        try {
          await desktopApi.revokeApiToken(stored.tokenId);
        } catch {
          // continue
        }
      }
      removeStoredMcpToken();
      setFixedMcpToken(null);
      setApiTokens((current) =>
        current.filter((token) => token.name !== "BugLogin MCP"),
      );
      showSuccessToast("Đã thu hồi Key MCP cố định.");
    } catch (cause) {
      showErrorToast(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setIsManagingMcpToken(false);
    }
  };

  const handlePing = async () => {
    setIsPinging(true);
    const start = performance.now();
    try {
      // Query through the native desktop bridge. Fetching localhost directly
      // from the WebView is subject to CORS and can report a false failure even
      // while the local API is healthy.
      const integration = await desktopApi.getIntegrationStatus();
      const elapsed = Math.round(performance.now() - start);
      if (integration.apiReachable) {
        setPingResult({
          latencyMs: elapsed,
          statusText: `200 OK (${integration.apiService || "buglogin-api"})`,
          timestamp: Date.now(),
        });
        showSuccessToast(`Kiểm tra API thành công (${elapsed}ms)`);
      } else {
        setPingResult({
          latencyMs: elapsed,
          statusText: "API unreachable",
          timestamp: Date.now(),
        });
        showErrorToast("Không thể kết nối đến máy chủ API cục bộ");
      }
    } catch (_err) {
      showErrorToast("Không thể kết nối đến máy chủ API cục bộ");
      setPingResult({
        latencyMs: 0,
        statusText: "Connection failed",
        timestamp: Date.now(),
      });
    } finally {
      setIsPinging(false);
    }
  };

  const codeSnippets: Record<CodeLang, { title: string; code: string }> =
    useMemo(
      () => ({
        curl: {
          title: "cURL / REST API",
          code: `# 1. Kiểm tra trạng thái máy chủ (Health Check)
curl ${endpoint}/v1/health

# 2. Lấy danh sách tất cả hồ sơ trình duyệt
curl ${endpoint}/v1/profiles

# 3. Khởi chạy một hồ sơ trình duyệt (trả về remote debug port)
curl -X POST ${endpoint}/v1/profiles/PROFILE_ID/launch

# 4. Đóng hồ sơ đang chạy
curl -X POST ${endpoint}/v1/profiles/PROFILE_ID/stop

# 5. Xuất Cookies của hồ sơ
curl ${endpoint}/v1/profiles/PROFILE_ID/cookies`,
        },
        node: {
          title: "Node.js (Puppeteer / Playwright)",
          code: `import puppeteer from "puppeteer-core";

const API_BASE = "${endpoint}";
const PROFILE_ID = "YOUR_PROFILE_ID"; // Thay bằng ID profile thực tế

// Bước 1: Gọi API khởi chạy Profile
const launchRes = await fetch(\`\${API_BASE}/v1/profiles/\${PROFILE_ID}/launch\`, {
  method: "POST",
});
const { debug_port, ws_endpoint } = await launchRes.json();

// Bước 2: Kết nối Puppeteer vào trình duyệt BugLogin qua CDP
const browser = await puppeteer.connect({
  browserWSEndpoint: ws_endpoint || \`ws://127.0.0.1:\${debug_port}\`,
});

const page = await browser.newPage();
await page.goto("https://facebook.com");
console.log("Đã tải trang thành công:", await page.title());

// Bước 3: Đóng hồ sơ khi hoàn thành tác vụ
// await fetch(\`\${API_BASE}/v1/profiles/\${PROFILE_ID}/stop\`, { method: "POST" });`,
        },
        python: {
          title: "Python (Playwright / Selenium)",
          code: `import requests
from playwright.sync_api import sync_playwright

API_BASE = "${endpoint}"
PROFILE_ID = "YOUR_PROFILE_ID"

# Bước 1: Khởi chạy Profile qua BugLogin Local API
resp = requests.post(f"{API_BASE}/v1/profiles/{PROFILE_ID}/launch")
data = resp.json()
debug_port = data.get("debug_port")

# Bước 2: Kết nối Playwright điều khiển trình duyệt qua CDP
with sync_playwright() as p:
    browser = p.chromium.connect_over_cdp(f"http://127.0.0.1:{debug_port}")
    context = browser.contexts[0]
    page = context.pages[0] if context.pages else context.new_page()
    page.goto("https://google.com")
    print("Tiêu đề trang:", page.title())

# Bước 3: Đóng profile khi xong việc
# requests.post(f"{API_BASE}/v1/profiles/{PROFILE_ID}/stop")`,
        },
      }),
      [endpoint],
    );

  const mcpConfigs: Record<
    McpClient,
    { name: string; file: string; config: string }
  > = useMemo(() => {
    const headers = {
      Authorization: `Bearer ${fixedMcpToken ?? "YOUR_BUGLOGIN_API_KEY"}`,
    };
    return {
      cursor: {
        name: "Cursor AI",
        file: ".cursor/mcp.json hoặc Cursor Settings -> Features -> MCP",
        config: JSON.stringify(
          {
            mcpServers: {
              buglogin: {
                url: mcpUrl,
                headers,
              },
            },
          },
          null,
          2,
        ),
      },
      claude: {
        name: "Claude Desktop",
        file: "%APPDATA%\\Claude\\claude_desktop_config.json",
        config: JSON.stringify(
          {
            mcpServers: {
              buglogin: {
                type: "http",
                url: mcpUrl,
                headers,
              },
            },
          },
          null,
          2,
        ),
      },
      antigravity: {
        name: "Antigravity / Windsurf",
        file: "~/.gemini/antigravity-cli/mcp_config.json hoặc .codeium/windsurf/mcp_config.json",
        config: JSON.stringify(
          {
            mcpServers: {
              buglogin: {
                url: mcpUrl,
                headers,
              },
            },
          },
          null,
          2,
        ),
      },
    };
  }, [mcpUrl, fixedMcpToken]);

  const apiEndpointsList = [
    {
      method: "GET",
      path: "/v1/health",
      desc: "Kiểm tra tình trạng hoạt động của máy chủ API cục bộ",
      color: "bg-emerald-500/15 text-emerald-500 border-emerald-500/30",
    },
    {
      method: "GET",
      path: "/v1/readyz",
      desc: "Kiểm tra mức độ sẵn sàng của các dịch vụ bên dưới",
      color: "bg-emerald-500/15 text-emerald-500 border-emerald-500/30",
    },
    {
      method: "GET",
      path: "/v1/profiles",
      desc: "Lấy danh sách tất cả các hồ sơ trình duyệt trong workspace",
      color: "bg-emerald-500/15 text-emerald-500 border-emerald-500/30",
    },
    {
      method: "POST",
      path: "/v1/profiles",
      desc: "Tạo mới một hồ sơ trình duyệt với cấu hình fingerprint tùy chỉnh",
      color: "bg-blue-500/15 text-blue-500 border-blue-500/30",
    },
    {
      method: "GET",
      path: "/v1/profiles/{id}",
      desc: "Xem chi tiết thông tin và cấu hình vân tay của một hồ sơ",
      color: "bg-emerald-500/15 text-emerald-500 border-emerald-500/30",
    },
    {
      method: "POST",
      path: "/v1/profiles/{id}/launch",
      desc: "Khởi chạy hồ sơ trình duyệt và nhận cổng điều khiển DevTools (CDP)",
      color: "bg-blue-500/15 text-blue-500 border-blue-500/30",
    },
    {
      method: "POST",
      path: "/v1/profiles/{id}/stop",
      desc: "Đóng và dọn dẹp tiến trình trình duyệt đang chạy",
      color: "bg-amber-500/15 text-amber-500 border-amber-500/30",
    },
    {
      method: "GET",
      path: "/v1/profiles/{id}/cookies",
      desc: "Xuất dữ liệu Cookie của hồ sơ dưới định dạng JSON",
      color: "bg-emerald-500/15 text-emerald-500 border-emerald-500/30",
    },
    {
      method: "POST",
      path: "/v1/profiles/{id}/clone",
      desc: "Nhân bản một hồ sơ trình duyệt hiện có",
      color: "bg-blue-500/15 text-blue-500 border-blue-500/30",
    },
    {
      method: "DELETE",
      path: "/v1/profiles/{id}",
      desc: "Xóa hồ sơ trình duyệt khỏi hệ thống",
      color: "bg-destructive/15 text-destructive border-destructive/30",
    },
    {
      method: "POST",
      path: "/v1/cookie-jobs",
      desc: "Tạo tác vụ Cookie Bot (tự động nuôi cookie / lướt web)",
      color: "bg-blue-500/15 text-blue-500 border-blue-500/30",
    },
    {
      method: "GET",
      path: "/v1/mcp",
      desc: "Điểm cuối giao thức Model Context Protocol dành cho AI Agent",
      color: "bg-purple-500/15 text-purple-400 border-purple-500/30",
    },
  ];

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      subPage={subPage}
    >
      <DialogContent
        className={cn(
          "flex max-h-[calc(100vh-3rem)] max-w-5xl flex-col p-6",
          subPage && "border-0 shadow-none bg-transparent p-0 max-w-full",
        )}
      >
        {/* Top Header */}
        <div className="flex flex-wrap items-center justify-between gap-4 border-b border-border/50 pb-4">
          <div className="flex items-center gap-3">
            <div className="grid size-11 place-items-center rounded-xl bg-primary/10 text-primary shadow-2xs">
              <LuPlug className="size-6 text-primary" />
            </div>
            <div>
              <div className="flex items-center gap-2.5">
                <DialogTitle className="text-base font-semibold">
                  {t("integrations.title", {
                    defaultValue: "Tích hợp & Tự động hóa",
                  })}
                </DialogTitle>
                <Badge
                  variant="outline"
                  className={cn(
                    "gap-1.5 px-2.5 py-0.5 text-xs font-medium",
                    isApiConnected
                      ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-500"
                      : "border-destructive/30 bg-destructive/10 text-destructive",
                  )}
                >
                  <span
                    className={cn(
                      "size-1.5 rounded-full",
                      isApiConnected
                        ? "bg-emerald-500 animate-pulse"
                        : "bg-destructive",
                    )}
                  />
                  <span>
                    {isApiConnected
                      ? t("integrations.status.connected", {
                          defaultValue: "Máy chủ đang chạy",
                        })
                      : t("integrations.status.disconnected", {
                          defaultValue: "Chưa kết nối",
                        })}
                  </span>
                </Badge>
              </div>
              <p className="text-xs text-muted-foreground mt-0.5 max-w-2xl">
                {t("integrations.description", {
                  defaultValue:
                    "Cung cấp REST API cục bộ và Model Context Protocol (MCP) để tự động hóa trình duyệt qua cURL, Puppeteer, Playwright, Selenium hoặc trợ lý AI.",
                })}
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="gap-2 h-8 text-xs cursor-pointer shadow-2xs"
              disabled={isPinging}
              onClick={() => void handlePing()}
            >
              <LuActivity
                className={cn(
                  "size-3.5 text-primary",
                  isPinging && "animate-spin",
                )}
              />
              <span>
                {isPinging
                  ? t("integrations.pinging", {
                      defaultValue: "Đang kiểm tra...",
                    })
                  : t("integrations.testConnection", {
                      defaultValue: "Kiểm tra kết nối",
                    })}
              </span>
            </Button>

            <Button
              type="button"
              size="sm"
              variant="secondary"
              className="gap-2 h-8 text-xs cursor-pointer shadow-2xs"
              disabled={loading}
              onClick={() => void refresh()}
            >
              <LuRefreshCw
                className={cn("size-3.5", loading && "animate-spin")}
              />
              <span>
                {t("common.buttons.refresh", { defaultValue: "Làm mới" })}
              </span>
            </Button>
          </div>
        </div>

        {error && (
          <div
            role="alert"
            className="my-3 rounded-lg border border-destructive/20 bg-destructive/10 px-4 py-2.5 text-xs text-destructive-text"
          >
            {error}
          </div>
        )}

        {/* Tab Switcher */}
        <div className="flex items-center justify-between gap-3 border-b border-border/40 pb-3 pt-2">
          <div className="flex rounded-lg bg-foreground/5 p-1" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={activeTab === "api"}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-3.5 py-1.5 text-xs font-medium transition-colors cursor-pointer",
                activeTab === "api"
                  ? "bg-background text-foreground shadow-xs font-semibold"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setActiveTab("api")}
            >
              <LuPlug className="size-3.5" />
              <span>
                {t("integrations.tabs.localApi", {
                  defaultValue: "API cục bộ (REST API)",
                })}
              </span>
            </button>

            <button
              type="button"
              role="tab"
              aria-selected={activeTab === "mcp"}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-3.5 py-1.5 text-xs font-medium transition-colors cursor-pointer",
                activeTab === "mcp"
                  ? "bg-background text-foreground shadow-xs font-semibold"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setActiveTab("mcp")}
            >
              <LuBot className="size-3.5" />
              <span>
                {t("integrations.tabs.mcp", {
                  defaultValue: "Trợ lý AI (MCP Protocol)",
                })}
              </span>
            </button>

            <button
              type="button"
              role="tab"
              aria-selected={activeTab === "endpoints"}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-3.5 py-1.5 text-xs font-medium transition-colors cursor-pointer",
                activeTab === "endpoints"
                  ? "bg-background text-foreground shadow-xs font-semibold"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setActiveTab("endpoints")}
            >
              <LuBookOpen className="size-3.5" />
              <span>
                {t("integrations.tabs.endpoints", {
                  defaultValue: "Danh mục Endpoints",
                })}
              </span>
            </button>
          </div>

          {pingResult && (
            <div className="hidden sm:flex items-center gap-2 text-[11px] text-muted-foreground">
              <span className="size-1.5 rounded-full bg-emerald-500" />
              <span>
                Phản hồi:{" "}
                <strong className="text-foreground">
                  {pingResult.latencyMs}ms
                </strong>
              </span>
              <span className="text-muted-foreground/60">•</span>
              <span className="font-mono text-emerald-500">
                {pingResult.statusText}
              </span>
            </div>
          )}
        </div>

        {/* Tab Content Body */}
        <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto pt-3 pr-1">
          {activeTab === "api" && (
            <div className="space-y-5">
              {/* Card 1: API Endpoint & Server Specs */}
              <div className="rounded-xl border border-border/70 bg-card p-5 shadow-2xs space-y-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="space-y-0.5">
                    <h3 className="text-sm font-semibold flex items-center gap-2">
                      <LuServer className="size-4 text-primary" />
                      <span>
                        {t("integrations.api.endpointTitle", {
                          defaultValue: "Điểm cuối API cục bộ (Local Endpoint)",
                        })}
                      </span>
                    </h3>
                    <p className="text-xs text-muted-foreground">
                      {t("integrations.api.endpointDesc", {
                        defaultValue:
                          "Máy chủ REST API chạy trên thiết bị, chấp nhận các lệnh điều khiển hồ sơ từ các tập lệnh tự động hóa.",
                      })}
                    </p>
                  </div>

                  <div className="flex items-center gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-8 gap-1.5 text-xs cursor-pointer"
                      onClick={() =>
                        void handleCopy(
                          endpoint,
                          "endpoint",
                          "Đã sao chép địa chỉ API",
                        )
                      }
                    >
                      {copiedKey === "endpoint" ? (
                        <LuCheck className="size-3.5 text-emerald-500" />
                      ) : (
                        <LuCopy className="size-3.5" />
                      )}
                      <span>
                        {copiedKey === "endpoint" ? "Đã chép" : "Sao chép URL"}
                      </span>
                    </Button>

                    <Button
                      variant="subtle"
                      size="sm"
                      className="h-8 gap-1.5 text-xs cursor-pointer"
                      onClick={() => void handlePing()}
                    >
                      <LuZap className="size-3.5 text-amber-500" />
                      <span>Ping Health</span>
                    </Button>
                  </div>
                </div>

                {/* Endpoint Code Box */}
                <div className="flex items-center justify-between rounded-lg bg-background/80 border border-border/60 px-4 py-2.5 font-mono text-xs">
                  <span className="font-semibold text-foreground tracking-wide selection:bg-primary/20">
                    {endpoint}
                  </span>
                  <Badge
                    variant="secondary"
                    className="text-[10px] uppercase font-mono tracking-wider"
                  >
                    HTTP REST
                  </Badge>
                </div>

                {/* Specs Grid */}
                <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 pt-1 text-xs">
                  <div className="rounded-lg bg-muted/40 p-3 border border-border/40 space-y-1">
                    <span className="text-muted-foreground text-[11px]">
                      Chế độ triển khai
                    </span>
                    <p className="font-medium text-foreground">
                      {status?.deploymentMode === "production"
                        ? "Sản xuất (Production)"
                        : "Cục bộ (Local Runtime)"}
                    </p>
                  </div>

                  <div className="rounded-lg bg-muted/40 p-3 border border-border/40 space-y-1">
                    <span className="text-muted-foreground text-[11px]">
                      Cổng dịch vụ (Port)
                    </span>
                    <p className="font-medium text-foreground font-mono">
                      {endpoint.split(":").pop()?.replace(/\D/g, "") || "12345"}
                    </p>
                  </div>

                  <div className="rounded-lg bg-muted/40 p-3 border border-border/40 space-y-1">
                    <span className="text-muted-foreground text-[11px]">
                      Định danh dịch vụ
                    </span>
                    <p className="font-medium text-foreground font-mono">
                      {status?.apiService || "buglogin-api"}
                    </p>
                  </div>
                </div>
              </div>

              {/* Card 2: Interactive Code Snippets */}
              <div className="rounded-xl border border-border/70 bg-card p-5 shadow-2xs space-y-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="space-y-0.5">
                    <h3 className="text-sm font-semibold flex items-center gap-2">
                      <LuCode className="size-4 text-primary" />
                      <span>
                        {t("integrations.quickstart.title", {
                          defaultValue: "Mẫu mã tích hợp nhanh (Quickstart)",
                        })}
                      </span>
                    </h3>
                    <p className="text-xs text-muted-foreground">
                      {t("integrations.quickstart.desc", {
                        defaultValue:
                          "Chọn ngôn ngữ hoặc framework tự động hóa để sao chép mã nguồn tích hợp sẵn vào dự án của bạn.",
                      })}
                    </p>
                  </div>

                  {/* Language switch */}
                  <div className="flex rounded-md bg-muted/60 p-0.5 border border-border/40">
                    {(["curl", "node", "python"] as const).map((lang) => (
                      <button
                        key={lang}
                        type="button"
                        onClick={() => setCodeLang(lang)}
                        className={cn(
                          "px-3 py-1 text-xs rounded transition-colors cursor-pointer capitalize font-medium",
                          codeLang === lang
                            ? "bg-background text-foreground shadow-2xs font-semibold"
                            : "text-muted-foreground hover:text-foreground",
                        )}
                      >
                        {lang === "node"
                          ? "Node.js (Puppeteer)"
                          : lang === "python"
                            ? "Python (Playwright)"
                            : "cURL"}
                      </button>
                    ))}
                  </div>
                </div>

                {/* Code Preview Box */}
                <div className="relative rounded-lg border border-border/60 bg-zinc-950 dark:bg-black p-4 text-zinc-100 font-mono text-xs overflow-x-auto shadow-inner">
                  <div className="absolute top-3 right-3 flex items-center gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7 px-2.5 text-[11px] gap-1.5 bg-zinc-900 border-zinc-700 text-zinc-200 hover:bg-zinc-800 hover:text-white cursor-pointer"
                      onClick={() =>
                        void handleCopy(
                          codeSnippets[codeLang].code,
                          `code_${codeLang}`,
                          `Đã sao chép code ${codeSnippets[codeLang].title}`,
                        )
                      }
                    >
                      {copiedKey === `code_${codeLang}` ? (
                        <LuCheck className="size-3 text-emerald-400" />
                      ) : (
                        <LuCopy className="size-3" />
                      )}
                      <span>
                        {copiedKey === `code_${codeLang}`
                          ? "Đã sao chép"
                          : "Sao chép code"}
                      </span>
                    </Button>
                  </div>

                  <pre className="pr-24 leading-relaxed font-mono">
                    <code>{codeSnippets[codeLang].code}</code>
                  </pre>
                </div>

                <div className="rounded-lg bg-primary/5 border border-primary/20 p-3 text-xs text-muted-foreground flex items-start gap-2.5">
                  <LuZap className="size-4 text-primary shrink-0 mt-0.5" />
                  <p className="leading-relaxed">
                    <strong>Gợi ý:</strong> Khi gọi API{" "}
                    <code className="font-mono text-foreground font-semibold">
                      /v1/profiles/:id/launch
                    </code>
                    , phản hồi sẽ trả về{" "}
                    <code className="font-mono text-foreground">
                      debug_port
                    </code>{" "}
                    và{" "}
                    <code className="font-mono text-foreground">
                      ws_endpoint
                    </code>
                    . Bạn có thể truyền trực tiếp vào Puppeteer, Playwright hoặc
                    Selenium để bắt đầu tự động hóa mà không cần cài đặt thêm
                    driver.
                  </p>
                </div>
              </div>
            </div>
          )}

          {activeTab === "mcp" && (
            <div className="space-y-5">
              {/* Card 1: MCP Intro & Status */}
              <div className="rounded-xl border border-border/70 bg-card p-5 shadow-2xs space-y-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="space-y-0.5">
                    <h3 className="text-sm font-semibold flex items-center gap-2">
                      <LuBot className="size-4 text-primary" />
                      <span>Model Context Protocol (MCP)</span>
                    </h3>
                    <p className="text-xs text-muted-foreground">
                      {t("integrations.mcp.desc", {
                        defaultValue:
                          "Chuẩn giao thức mở của Anthropic cho phép các trợ lý AI (Cursor, Claude Desktop, Antigravity, Windsurf) trực tiếp mở browser, tự động hóa tác vụ trên BugLogin.",
                      })}
                    </p>
                  </div>

                  <Badge
                    variant="outline"
                    className={cn(
                      "gap-1.5 text-xs px-2.5 py-0.5 font-medium",
                      fixedMcpToken
                        ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-400"
                        : mcpEnabled
                          ? "border-amber-500/30 bg-amber-500/10 text-amber-400"
                          : "border-border bg-muted text-muted-foreground",
                    )}
                  >
                    <span
                      className={cn(
                        "size-1.5 rounded-full",
                        fixedMcpToken
                          ? "bg-emerald-400"
                          : mcpEnabled
                            ? "bg-amber-400"
                            : "bg-muted-foreground",
                      )}
                    />
                    <span>
                      {fixedMcpToken
                        ? "MCP sẵn sàng kết nối"
                        : mcpEnabled
                          ? "Cần tạo API key cố định"
                          : "MCP chưa được bật"}
                    </span>
                  </Badge>
                </div>

                <p className="text-xs text-muted-foreground">
                  MCP yêu cầu scoped API key của account đang đăng nhập. Chưa có
                  key thì cấu hình client bên dưới chưa thể kết nối.
                </p>

                {/* Khối API Key Cố Định Cho MCP */}
                <div className="space-y-3.5 rounded-lg border border-border/70 bg-background/60 p-4 shadow-2xs">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <div className="space-y-0.5">
                      <div className="flex items-center gap-2">
                        <LuKey className="size-4 text-primary" />
                        <p className="text-sm font-semibold text-foreground">
                          API key cố định cho MCP
                        </p>
                        {fixedMcpToken ? (
                          <Badge
                            variant="outline"
                            className="text-[11px] px-2 py-0 h-5 border-emerald-500/30 bg-emerald-500/10 text-emerald-400 font-medium"
                          >
                            <span className="size-1.5 rounded-full bg-emerald-400 mr-1.5" />
                            Đang hoạt động
                          </Badge>
                        ) : (
                          <Badge
                            variant="outline"
                            className="text-[11px] px-2 py-0 h-5 border-amber-500/30 bg-amber-500/10 text-amber-400 font-medium"
                          >
                            <span className="size-1.5 rounded-full bg-amber-400 mr-1.5" />
                            Chưa có key
                          </Badge>
                        )}
                      </div>
                      <p className="text-xs text-muted-foreground">
                        Key cố định dùng kết nối các trợ lý AI (Cursor, Claude,
                        Windsurf, Antigravity). Được lưu an toàn trên máy này.
                      </p>
                    </div>

                    {!fixedMcpToken && activeMcpTokens.length === 0 && (
                      <Button
                        size="sm"
                        disabled={isManagingMcpToken || !isApiConnected}
                        onClick={() => void handleCreateOrRegenerateMcpToken()}
                        className="h-8 gap-1.5 cursor-pointer font-medium"
                      >
                        <LuKey className="size-3.5" />
                        {isManagingMcpToken
                          ? "Đang tạo…"
                          : "Tạo key MCP cố định"}
                      </Button>
                    )}
                  </div>

                  {/* Case 1: Đã có key cố định và chuỗi token được lưu */}
                  {fixedMcpToken ? (
                    <div className="space-y-3">
                      <div className="flex flex-wrap items-center gap-2">
                        {/* Masked / Plaintext Key Box */}
                        <div className="relative min-w-0 flex-1 flex items-center rounded-md border border-border/80 bg-muted/50 px-3 py-1.5 font-mono text-xs">
                          <span className="min-w-0 flex-1 select-all break-all tracking-wide text-foreground font-semibold">
                            {showMcpKey
                              ? fixedMcpToken
                              : `blapi_${"•".repeat(32)}`}
                          </span>
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="h-7 w-7 p-0 ml-1.5 text-muted-foreground hover:text-foreground cursor-pointer shrink-0"
                            onClick={() => setShowMcpKey(!showMcpKey)}
                            title={showMcpKey ? "Ẩn API key" : "Hiện API key"}
                          >
                            {showMcpKey ? (
                              <LuEyeOff className="size-3.5" />
                            ) : (
                              <LuEye className="size-3.5" />
                            )}
                            <span className="sr-only">
                              {showMcpKey ? "Ẩn" : "Hiện"}
                            </span>
                          </Button>
                        </div>

                        {/* Copy Button */}
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-9 gap-1.5 text-xs cursor-pointer font-medium"
                          onClick={() =>
                            void handleCopy(
                              fixedMcpToken,
                              "mcp_fixed_token",
                              "Đã sao chép API key MCP vào bộ nhớ tạm",
                            )
                          }
                        >
                          {copiedKey === "mcp_fixed_token" ? (
                            <LuCheck className="size-3.5 text-emerald-500" />
                          ) : (
                            <LuCopy className="size-3.5" />
                          )}
                          <span>
                            {copiedKey === "mcp_fixed_token"
                              ? "Đã chép"
                              : "Sao chép key"}
                          </span>
                        </Button>

                        {/* Regenerate Key */}
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={isManagingMcpToken}
                          className="h-9 gap-1.5 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
                          onClick={() =>
                            void handleCreateOrRegenerateMcpToken()
                          }
                          title="Tạo lại key mới (thu hồi key cũ)"
                        >
                          <LuRefreshCw
                            className={cn(
                              "size-3.5",
                              isManagingMcpToken && "animate-spin",
                            )}
                          />
                          <span>Tạo lại key</span>
                        </Button>

                        {/* Revoke Key */}
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={isManagingMcpToken}
                          className="h-9 gap-1.5 text-xs text-rose-400 hover:text-rose-300 hover:bg-rose-500/10 cursor-pointer"
                          onClick={() => void handleRevokeFixedMcpToken()}
                          title="Thu hồi key MCP"
                        >
                          <LuTrash2 className="size-3.5" />
                          <span>Thu hồi</span>
                        </Button>
                      </div>

                      {/* Token Details Note */}
                      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground px-0.5">
                        <span>
                          {primaryMcpRecord?.expiresAt
                            ? `Hết hạn: ${new Date(primaryMcpRecord.expiresAt * 1000).toLocaleDateString()}`
                            : "Thời hạn: 1 năm"}{" "}
                          · Quyền: profile, sync
                        </span>
                        <span className="text-emerald-500/90 flex items-center gap-1 font-medium">
                          <LuCheck className="size-3" /> Tự động điền vào cấu
                          hình bên dưới
                        </span>
                      </div>
                    </div>
                  ) : activeMcpTokens.length > 0 ? (
                    /* Case 2: Đang có key cũ trên tài khoản nhưng chưa lưu secret trên máy này */
                    <div className="rounded-md border border-amber-500/30 bg-amber-500/10 p-3.5 space-y-3">
                      <div className="flex items-start gap-2.5 text-xs text-amber-300">
                        <LuTriangleAlert className="size-4 shrink-0 mt-0.5" />
                        <div className="space-y-1">
                          <p className="font-semibold text-amber-200">
                            Phát hiện {activeMcpTokens.length} key MCP cũ trên
                            tài khoản (Chưa lưu mã bí mật trên máy này)
                          </p>
                          <p className="text-amber-300/80 leading-relaxed text-[11px]">
                            Chuỗi bí mật chưa được lưu cố định trên thiết bị này
                            nên không thể Hiện hay Sao chép trực tiếp. Hãy bấm
                            nút bên dưới để dọn dẹp các key cũ và kích hoạt 1
                            Key cố định duy nhất.
                          </p>
                        </div>
                      </div>
                      <div className="flex flex-wrap items-center gap-2">
                        <Button
                          size="sm"
                          disabled={isManagingMcpToken}
                          className="h-8 gap-1.5 text-xs cursor-pointer font-medium"
                          onClick={() =>
                            void handleCreateOrRegenerateMcpToken()
                          }
                        >
                          <LuSparkles className="size-3.5" />
                          {isManagingMcpToken
                            ? "Đang xử lý…"
                            : "Kích hoạt Key cố định duy nhất"}
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={isManagingMcpToken}
                          className="h-8 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
                          onClick={() => void handleRevokeFixedMcpToken()}
                        >
                          Thu hồi các key cũ
                        </Button>
                      </div>
                    </div>
                  ) : (
                    /* Case 3: Chưa có key nào */
                    <div className="rounded-md border border-dashed border-border/80 bg-muted/20 p-4 text-center space-y-2.5">
                      <p className="text-xs text-muted-foreground">
                        Chưa có Key MCP nào được tạo. Tạo một key cố định duy
                        nhất để bắt đầu tự động hóa với các trợ lý AI.
                      </p>
                      <Button
                        size="sm"
                        disabled={isManagingMcpToken || !isApiConnected}
                        onClick={() => void handleCreateOrRegenerateMcpToken()}
                        className="h-8 gap-1.5 cursor-pointer font-medium"
                      >
                        <LuKey className="size-3.5" />
                        {isManagingMcpToken
                          ? "Đang tạo…"
                          : "Tạo key MCP cố định"}
                      </Button>
                    </div>
                  )}
                </div>

                <div className="flex items-center justify-between rounded-lg bg-background/80 border border-border/60 px-4 py-2.5 font-mono text-xs">
                  <div className="space-y-0.5">
                    <span className="text-[10px] text-muted-foreground uppercase font-sans font-semibold">
                      MCP Server Endpoint
                    </span>
                    <p className="font-semibold text-foreground tracking-wide">
                      {mcpUrl}
                    </p>
                  </div>

                  <Button
                    variant="outline"
                    size="sm"
                    className="h-8 gap-1.5 text-xs cursor-pointer"
                    onClick={() =>
                      void handleCopy(mcpUrl, "mcp_url", "Đã sao chép MCP URL")
                    }
                  >
                    {copiedKey === "mcp_url" ? (
                      <LuCheck className="size-3.5 text-emerald-500" />
                    ) : (
                      <LuCopy className="size-3.5" />
                    )}
                    <span>
                      {copiedKey === "mcp_url" ? "Đã chép" : "Sao chép URL"}
                    </span>
                  </Button>
                </div>
              </div>

              {/* Card 2: AI Clients Configuration */}
              <div className="rounded-xl border border-border/70 bg-card p-5 shadow-2xs space-y-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="space-y-0.5">
                    <h3 className="text-sm font-semibold flex items-center gap-2">
                      <LuCpu className="size-4 text-primary" />
                      <span>
                        {t("integrations.mcp.clientsTitle", {
                          defaultValue: "Cấu hình AI Agent (1-Click Setup)",
                        })}
                      </span>
                    </h3>
                    <p className="text-xs text-muted-foreground">
                      Dán đoạn mã cấu hình dưới đây vào file cài đặt MCP của ứng
                      dụng AI bạn đang dùng.
                    </p>
                  </div>

                  {/* Client tabs */}
                  <div className="flex rounded-md bg-muted/60 p-0.5 border border-border/40">
                    {(["cursor", "claude", "antigravity"] as const).map(
                      (client) => (
                        <button
                          key={client}
                          type="button"
                          onClick={() => setMcpClient(client)}
                          className={cn(
                            "px-3 py-1 text-xs rounded transition-colors cursor-pointer font-medium",
                            mcpClient === client
                              ? "bg-background text-foreground shadow-2xs font-semibold"
                              : "text-muted-foreground hover:text-foreground",
                          )}
                        >
                          {mcpConfigs[client].name}
                        </button>
                      ),
                    )}
                  </div>
                </div>

                <div className="space-y-2">
                  <div className="flex items-center justify-between text-xs text-muted-foreground">
                    <span>
                      Đường dẫn tệp cấu hình:{" "}
                      <code className="font-mono text-foreground font-medium">
                        {mcpConfigs[mcpClient].file}
                      </code>
                    </span>
                  </div>

                  <div className="relative rounded-lg border border-border/60 bg-zinc-950 dark:bg-black p-4 text-zinc-100 font-mono text-xs overflow-x-auto shadow-inner">
                    <div className="absolute top-3 right-3">
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-7 px-2.5 text-[11px] gap-1.5 bg-zinc-900 border-zinc-700 text-zinc-200 hover:bg-zinc-800 hover:text-white cursor-pointer"
                        onClick={() =>
                          void handleCopy(
                            mcpConfigs[mcpClient].config,
                            `mcp_conf_${mcpClient}`,
                            fixedMcpToken
                              ? `Đã sao chép cấu hình ${mcpConfigs[mcpClient].name}`
                              : `Đã sao chép cấu hình mẫu (chưa có key)`,
                          )
                        }
                      >
                        {copiedKey === `mcp_conf_${mcpClient}` ? (
                          <LuCheck className="size-3 text-emerald-400" />
                        ) : (
                          <LuCopy className="size-3" />
                        )}
                        <span>
                          {copiedKey === `mcp_conf_${mcpClient}`
                            ? "Đã sao chép"
                            : "Sao chép JSON"}
                        </span>
                      </Button>
                    </div>

                    <pre className="pr-24 leading-relaxed font-mono">
                      <code>{mcpConfigs[mcpClient].config}</code>
                    </pre>
                  </div>

                  {fixedMcpToken ? (
                    <p className="text-[11px] text-emerald-400 font-medium flex items-center gap-1.5 pt-0.5">
                      <LuCheck className="size-3.5" />
                      <span>
                        Đã tự động điền Key MCP cố định của bạn vào file cấu
                        hình. Chỉ cần sao chép và dán vào ứng dụng AI.
                      </span>
                    </p>
                  ) : (
                    <p className="text-[11px] text-amber-400/90 font-medium flex items-center gap-1.5 pt-0.5">
                      <LuTriangleAlert className="size-3.5" />
                      <span>
                        Chưa có Key MCP cố định. Vui lòng bấm &quot;Tạo key MCP
                        cố định&quot; ở trên để tự động điền key thật.
                      </span>
                    </p>
                  )}
                </div>

                <div className="rounded-lg bg-muted/40 p-3.5 border border-border/40 space-y-1.5 text-xs">
                  <span className="font-semibold text-foreground flex items-center gap-1.5">
                    <LuShieldCheck className="size-4 text-emerald-500" />
                    <span>Các công cụ MCP được cung cấp tự động:</span>
                  </span>
                  <ul className="list-disc pl-5 space-y-1 text-muted-foreground">
                    <li>
                      <strong className="text-foreground">
                        list_profiles:
                      </strong>{" "}
                      Liệt kê danh sách hồ sơ trình duyệt, proxy và tags
                    </li>
                    <li>
                      <strong className="text-foreground">
                        create_profile:
                      </strong>{" "}
                      Tự động tạo hồ sơ với OS và fingerprint giả lập
                    </li>
                    <li>
                      <strong className="text-foreground">
                        launch_profile:
                      </strong>{" "}
                      Mở trình duyệt và bàn giao quyền điều khiển tự động cho AI
                    </li>
                    <li>
                      <strong className="text-foreground">stop_profile:</strong>{" "}
                      Dừng và giải phóng tài nguyên trình duyệt
                    </li>
                  </ul>
                </div>
              </div>
            </div>
          )}

          {activeTab === "endpoints" && (
            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-sm font-semibold flex items-center gap-2">
                    <LuLayers className="size-4 text-primary" />
                    <span>
                      {t("integrations.endpoints.title", {
                        defaultValue: "Danh mục Endpoints chính thức",
                      })}
                    </span>
                  </h3>
                  <p className="text-xs text-muted-foreground mt-0.5">
                    Tất cả các tuyến đường API đều hỗ trợ giao thức HTTP REST,
                    nhận và trả về định dạng chuẩn JSON.
                  </p>
                </div>
              </div>

              <div className="rounded-xl border border-border/70 bg-card overflow-hidden shadow-2xs">
                <div className="overflow-x-auto">
                  <table className="w-full text-left text-xs">
                    <thead className="bg-muted/50 border-b border-border/60 text-muted-foreground font-medium">
                      <tr>
                        <th className="py-2.5 px-4 w-28">Phương thức</th>
                        <th className="py-2.5 px-4 w-72">Đường dẫn (Path)</th>
                        <th className="py-2.5 px-4">Mô tả chức năng</th>
                        <th className="py-2.5 px-4 w-20 text-right">
                          Thao tác
                        </th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-border/40">
                      {apiEndpointsList.map((ep, idx) => (
                        <tr
                          key={idx}
                          className="hover:bg-muted/30 transition-colors"
                        >
                          <td className="py-2.5 px-4">
                            <span
                              className={cn(
                                "inline-block px-2 py-0.5 rounded text-[10px] font-mono font-bold border",
                                ep.color,
                              )}
                            >
                              {ep.method}
                            </span>
                          </td>
                          <td className="py-2.5 px-4 font-mono font-medium text-foreground">
                            {ep.path}
                          </td>
                          <td className="py-2.5 px-4 text-muted-foreground leading-relaxed">
                            {ep.desc}
                          </td>
                          <td className="py-2.5 px-4 text-right">
                            <Button
                              variant="subtle"
                              size="icon"
                              className="size-7 rounded p-0 text-muted-foreground hover:text-foreground cursor-pointer"
                              title="Sao chép đường dẫn"
                              onClick={() =>
                                void handleCopy(
                                  ep.path,
                                  `ep_${idx}`,
                                  `Đã sao chép ${ep.path}`,
                                )
                              }
                            >
                              {copiedKey === `ep_${idx}` ? (
                                <LuCheck className="size-3.5 text-emerald-500" />
                              ) : (
                                <LuCopy className="size-3.5" />
                              )}
                            </Button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
