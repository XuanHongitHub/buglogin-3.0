"use client";

import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { useCallback, useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCheck,
  LuChevronsUpDown,
  LuClipboard,
  LuGlobe,
  LuList,
  LuPlus,
  LuTrash2,
} from "react-icons/lu";
import { toast } from "sonner";
import { LoadingButton } from "@/components/loading-button";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import type { BrowserProfile, StoredProxy } from "@/types";
import { RippleButton } from "./ui/ripple";

interface ParsedProxy {
  proxy_type: "http" | "socks5";
  host: string;
  port: number;
  username?: string;
  password?: string;
  raw: string;
}

function parseRawProxy(input: string): ParsedProxy | null {
  const trimmed = input.trim();
  if (!trimmed) return null;

  try {
    if (trimmed.includes("://")) {
      const url = new URL(trimmed);
      const isSocks = url.protocol.startsWith("socks");
      return {
        proxy_type: isSocks ? "socks5" : "http",
        host: url.hostname,
        port: parseInt(url.port, 10) || (isSocks ? 1080 : 8080),
        username: url.username ? decodeURIComponent(url.username) : undefined,
        password: url.password ? decodeURIComponent(url.password) : undefined,
        raw: trimmed,
      };
    }
  } catch {
    // fallback
  }

  const parts = trimmed.split(":");
  if (parts.length >= 2) {
    const host = parts[0];
    const port = parseInt(parts[1], 10);
    if (!isNaN(port) && host.length > 0) {
      return {
        proxy_type: "http",
        host,
        port,
        username: parts[2] || undefined,
        password: parts[3] || undefined,
        raw: trimmed,
      };
    }
  }

  return null;
}

interface ProxyAssignmentDialogProps {
  isOpen: boolean;
  onClose: () => void;
  selectedProfiles: string[];
  onAssignmentComplete: () => void;
  profiles?: BrowserProfile[];
  storedProxies?: StoredProxy[];
}

export function ProxyAssignmentDialog({
  isOpen,
  onClose,
  selectedProfiles,
  onAssignmentComplete,
  profiles = [],
  storedProxies = [],
}: ProxyAssignmentDialogProps) {
  const { t } = useTranslation();
  const proxyListboxId = useId();

  // Mode: "paste" (direct input/batch), "saved" (select stored), "remove" (detach)
  const [mode, setMode] = useState<"paste" | "saved" | "remove">("paste");

  // Direct paste state
  const [rawText, setRawText] = useState("");
  const [distributionMode, setDistributionMode] = useState<
    "sequential" | "first_for_all"
  >("sequential");

  // Saved selector state
  const [selectedStoredId, setSelectedStoredId] = useState<string | null>(null);
  const [proxyPopoverOpen, setProxyPopoverOpen] = useState(false);

  const [isAssigning, setIsAssigning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Parse raw proxies whenever rawText changes
  const parsedProxies: ParsedProxy[] = rawText
    .split(/\r?\n/)
    .map((line) => parseRawProxy(line))
    .filter((p): p is ParsedProxy => p !== null);

  const handlePasteClipboard = async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text) {
        setRawText((prev) => (prev ? `${prev}\n${text.trim()}` : text.trim()));
      }
    } catch {
      toast.error(
        t("proxies.clipboardReadFailed", {
          defaultValue: "Không thể đọc từ clipboard",
        }),
      );
    }
  };

  const handleAssign = useCallback(async () => {
    setIsAssigning(true);
    setError(null);

    try {
      const validProfiles = selectedProfiles.filter((profileId) =>
        profiles.some((p) => p.id === profileId),
      );

      if (validProfiles.length === 0) {
        setError(
          t("proxyAssignment.noValidProfiles", {
            defaultValue: "Không có profile hợp lệ",
          }),
        );
        setIsAssigning(false);
        return;
      }

      if (mode === "remove") {
        for (const profileId of validProfiles) {
          await invoke("update_profile_proxy", { profileId, proxyId: null });
        }
        await emit("profile-updated");
        toast.success(
          t("proxyAssignment.detachedSuccess", {
            defaultValue: `Đã gỡ proxy cho ${validProfiles.length} profile`,
          }),
        );
        onAssignmentComplete();
        onClose();
        return;
      }

      if (mode === "saved") {
        if (!selectedStoredId) {
          setError(
            t("proxyAssignment.noProxySelected", {
              defaultValue: "Vui lòng chọn proxy",
            }),
          );
          setIsAssigning(false);
          return;
        }

        for (const profileId of validProfiles) {
          await invoke("update_profile_proxy", {
            profileId,
            proxyId: selectedStoredId,
          });
        }

        await emit("profile-updated");
        toast.success(
          t("proxyAssignment.assignedSuccess", {
            defaultValue: `Đã gán proxy cho ${validProfiles.length} profile`,
          }),
        );
        onAssignmentComplete();
        onClose();
        return;
      }

      // mode === "paste"
      if (parsedProxies.length === 0) {
        setError(
          t("proxyAssignment.invalidFormatNotice", {
            defaultValue:
              "Vui lòng nhập ít nhất 1 proxy hợp lệ (host:port hoặc host:port:user:pass)",
          }),
        );
        setIsAssigning(false);
        return;
      }

      // Create stored proxies and assign
      for (let i = 0; i < validProfiles.length; i++) {
        const profileId = validProfiles[i];
        const proxyToUse =
          distributionMode === "first_for_all"
            ? parsedProxies[0]
            : parsedProxies[i % parsedProxies.length];

        const proxyId = `proxy_${crypto.randomUUID()}`;
        await invoke("create_stored_proxy", {
          name: `${proxyToUse.host}:${proxyToUse.port}`,
          proxySettings: {
            proxy_type: proxyToUse.proxy_type,
            host: proxyToUse.host,
            port: proxyToUse.port,
            username: proxyToUse.username,
            password: proxyToUse.password,
          },
          idempotencyKey: crypto.randomUUID(),
          proxyId,
          createdAt: new Date().toISOString(),
        });

        await invoke("update_profile_proxy", {
          profileId,
          proxyId,
        });
      }

      await emit("stored-proxies-changed");
      await emit("profile-updated");

      toast.success(
        t("proxyAssignment.assignedSuccess", {
          defaultValue: `Đã tạo và gán proxy cho ${validProfiles.length} profile`,
        }),
      );
      onAssignmentComplete();
      onClose();
    } catch (err) {
      console.error("Failed to assign proxy to profiles:", err);
      const errorMessage =
        err instanceof Error
          ? err.message
          : t("proxyAssignment.failedFallback");
      setError(errorMessage);
      toast.error(errorMessage);
    } finally {
      setIsAssigning(false);
    }
  }, [
    selectedProfiles,
    profiles,
    mode,
    selectedStoredId,
    parsedProxies,
    distributionMode,
    onAssignmentComplete,
    onClose,
    t,
  ]);

  useEffect(() => {
    if (isOpen) {
      setMode("paste");
      setRawText("");
      setSelectedStoredId(null);
      setError(null);
    }
  }, [isOpen]);

  const targetProfileCount = selectedProfiles.length;

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="max-w-lg p-5">
        <DialogHeader>
          <div className="flex items-center gap-2">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <LuGlobe className="size-4" />
            </div>
            <div>
              <DialogTitle className="text-base font-semibold">
                {t("proxyAssignment.title", {
                  defaultValue: "Cấu hình Proxy cho Profile",
                })}
              </DialogTitle>
              <DialogDescription className="text-xs">
                {targetProfileCount === 1
                  ? t("proxyAssignment.description_one", {
                      count: targetProfileCount,
                      defaultValue: "Cấu hình proxy cho 1 profile đã chọn",
                    })
                  : t("proxyAssignment.description_other", {
                      count: targetProfileCount,
                      defaultValue: `Cấu hình hoặc phân bổ proxy cho ${targetProfileCount} profile đã chọn`,
                    })}
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        {/* Selected profiles badge summary */}
        <div className="rounded-md border border-border/60 bg-muted/30 p-2.5">
          <div className="flex items-center justify-between text-xs text-muted-foreground mb-1.5 font-medium">
            <span>
              {t("proxyAssignment.selectedProfilesLabel", {
                defaultValue: "Danh sách profiles áp dụng",
              })}{" "}
              ({targetProfileCount}):
            </span>
          </div>
          <div className="max-h-16 overflow-y-auto flex flex-wrap gap-1.5">
            {selectedProfiles.map((profileId) => {
              const p = profiles.find((item) => item.id === profileId);
              return (
                <span
                  key={profileId}
                  className="inline-flex items-center gap-1 rounded bg-secondary px-2 py-0.5 text-[11px] font-medium text-secondary-foreground"
                >
                  <span className="size-1.5 rounded-full bg-primary/70" />
                  {p ? p.name : profileId}
                </span>
              );
            })}
          </div>
        </div>

        {/* Mode Selector Tabs */}
        <div className="grid grid-cols-3 gap-1 rounded-lg bg-muted p-1 text-xs">
          <button
            type="button"
            onClick={() => setMode("paste")}
            className={cn(
              "flex items-center justify-center gap-1.5 rounded-md py-1.5 font-medium transition-all",
              mode === "paste"
                ? "bg-background text-foreground shadow-xs"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            <LuPlus className="size-3.5" />
            <span>
              {t("proxies.quickInput", { defaultValue: "Dán IP mới" })}
            </span>
          </button>
          <button
            type="button"
            onClick={() => setMode("saved")}
            className={cn(
              "flex items-center justify-center gap-1.5 rounded-md py-1.5 font-medium transition-all",
              mode === "saved"
                ? "bg-background text-foreground shadow-xs"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            <LuList className="size-3.5" />
            <span>
              {t("proxies.savedTab", { defaultValue: "Proxy đã lưu" })}
            </span>
          </button>
          <button
            type="button"
            onClick={() => setMode("remove")}
            className={cn(
              "flex items-center justify-center gap-1.5 rounded-md py-1.5 font-medium transition-all",
              mode === "remove"
                ? "bg-destructive/15 text-destructive shadow-xs font-semibold"
                : "text-muted-foreground hover:text-destructive",
            )}
          >
            <LuTrash2 className="size-3.5" />
            <span>{t("proxies.removeTab", { defaultValue: "Gỡ Proxy" })}</span>
          </button>
        </div>

        {/* Mode = "paste" */}
        {mode === "paste" && (
          <div className="space-y-3">
            <div className="flex items-center justify-between text-xs">
              <span className="text-muted-foreground">
                {t("proxies.formatTip", {
                  defaultValue:
                    "Hỗ trợ host:port:user:pass hoặc URL (mỗi dòng 1 proxy):",
                })}
              </span>
              <Button
                variant="ghost"
                size="sm"
                className="h-6 px-2 text-[11px] gap-1 text-primary hover:text-primary"
                onClick={handlePasteClipboard}
              >
                <LuClipboard className="size-3" />
                <span>
                  {t("proxies.pasteClipboard", {
                    defaultValue: "Dán từ Clipboard",
                  })}
                </span>
              </Button>
            </div>

            <Textarea
              rows={4}
              value={rawText}
              onChange={(e) => setRawText(e.target.value)}
              placeholder="192.168.1.10:8080:user:pass&#10;socks5://user:pass@192.168.1.11:1080&#10;14.225.1.2:3128"
              className="font-mono text-xs leading-relaxed"
            />

            <div className="flex items-center justify-between text-xs">
              <span
                className={cn(
                  "font-medium",
                  parsedProxies.length > 0
                    ? "text-emerald-500"
                    : "text-muted-foreground",
                )}
              >
                {parsedProxies.length > 0 ? (
                  <>
                    ✓{" "}
                    {t("proxies.validCount", {
                      count: parsedProxies.length,
                      defaultValue: `Đã nhận diện: ${parsedProxies.length} proxy hợp lệ`,
                    })}
                  </>
                ) : (
                  t("proxies.noProxyDetected", {
                    defaultValue: "Chưa nhận diện được proxy hợp lệ",
                  })
                )}
              </span>

              {targetProfileCount > 1 && parsedProxies.length > 1 && (
                <div className="flex items-center gap-2 text-[11px]">
                  <label className="flex items-center gap-1 cursor-pointer">
                    <input
                      type="radio"
                      name="distMode"
                      checked={distributionMode === "sequential"}
                      onChange={() => setDistributionMode("sequential")}
                      className="size-3 text-primary"
                    />
                    <span>
                      {t("proxies.distSequential", {
                        defaultValue: "1 proxy / profile",
                      })}
                    </span>
                  </label>
                  <label className="flex items-center gap-1 cursor-pointer">
                    <input
                      type="radio"
                      name="distMode"
                      checked={distributionMode === "first_for_all"}
                      onChange={() => setDistributionMode("first_for_all")}
                      className="size-3 text-primary"
                    />
                    <span>
                      {t("proxies.distFirstForAll", {
                        defaultValue: "Dùng chung proxy 1",
                      })}
                    </span>
                  </label>
                </div>
              )}
            </div>
          </div>
        )}

        {/* Mode = "saved" */}
        {mode === "saved" && (
          <div className="space-y-3">
            <Label htmlFor="proxy-vpn-select" className="text-xs">
              {t("proxyAssignment.assignProxyVpnLabel", {
                defaultValue: "Chọn Proxy đã lưu trong hệ thống:",
              })}
            </Label>
            <Popover open={proxyPopoverOpen} onOpenChange={setProxyPopoverOpen}>
              <PopoverTrigger asChild>
                <Button
                  variant="outline"
                  role="combobox"
                  aria-expanded={proxyPopoverOpen}
                  aria-controls={proxyListboxId}
                  className="w-full justify-between font-normal text-xs"
                >
                  {(() => {
                    if (!selectedStoredId)
                      return t("proxyAssignment.noneOption", {
                        defaultValue: "Chọn proxy...",
                      });
                    const proxy = storedProxies.find(
                      (p) => p.id === selectedStoredId,
                    );
                    return proxy
                      ? `${proxy.name} (${proxy.proxy_settings.host}:${proxy.proxy_settings.port})`
                      : selectedStoredId;
                  })()}
                  <LuChevronsUpDown className="ml-2 size-4 shrink-0 opacity-50" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                id={proxyListboxId}
                className="w-(--radix-popover-trigger-width) p-0"
                sideOffset={8}
              >
                <Command>
                  <CommandInput
                    placeholder={t("proxyAssignment.searchPlaceholder", {
                      defaultValue: "Tìm proxy đã lưu...",
                    })}
                  />
                  <CommandList className="max-h-48">
                    <CommandEmpty>
                      {t("proxyAssignment.notFound", {
                        defaultValue: "Không tìm thấy proxy",
                      })}
                    </CommandEmpty>
                    <CommandGroup
                      heading={t("proxies.savedHeading", {
                        defaultValue: "Proxy đã lưu",
                      })}
                    >
                      {storedProxies
                        .filter(
                          (proxy) =>
                            !proxy.is_cloud_managed && !proxy.is_cloud_derived,
                        )
                        .map((proxy) => (
                          <CommandItem
                            key={proxy.id}
                            value={`${proxy.name} ${proxy.proxy_settings.host}:${proxy.proxy_settings.port}`}
                            onSelect={() => {
                              setSelectedStoredId(proxy.id);
                              setProxyPopoverOpen(false);
                            }}
                            className="text-xs flex items-center justify-between"
                          >
                            <div className="flex items-center gap-2">
                              <LuCheck
                                className={cn(
                                  "size-3.5",
                                  selectedStoredId === proxy.id
                                    ? "opacity-100 text-primary"
                                    : "opacity-0",
                                )}
                              />
                              <span className="font-medium">{proxy.name}</span>
                              <span className="text-muted-foreground font-mono text-[11px]">
                                {proxy.proxy_settings.host}:
                                {proxy.proxy_settings.port}
                              </span>
                            </div>
                            <Badge
                              variant="outline"
                              className="text-[10px] uppercase"
                            >
                              {proxy.proxy_settings.proxy_type}
                            </Badge>
                          </CommandItem>
                        ))}
                    </CommandGroup>
                  </CommandList>
                </Command>
              </PopoverContent>
            </Popover>
          </div>
        )}

        {/* Mode = "remove" */}
        {mode === "remove" && (
          <div className="rounded-lg border border-destructive/20 bg-destructive/10 p-3 text-xs text-destructive-foreground">
            <p className="font-medium mb-1">
              {t("proxies.confirmDetachTitle", {
                defaultValue: "Xác nhận gỡ bỏ Proxy",
              })}
            </p>
            <p className="text-muted-foreground text-[11px]">
              {t("proxies.confirmDetachDesc", {
                defaultValue: `Proxy sẽ được gỡ khỏi ${targetProfileCount} profile đã chọn. Trình duyệt sẽ kết nối trực tiếp qua mạng Internet cục bộ của máy tính.`,
              })}
            </p>
          </div>
        )}

        {error && (
          <div className="rounded-md bg-destructive/10 p-2.5 text-xs text-destructive-text">
            {error}
          </div>
        )}

        <DialogFooter className="mt-2">
          <RippleButton
            variant="outline"
            onClick={onClose}
            disabled={isAssigning}
            className="text-xs"
          >
            {t("common.buttons.cancel", { defaultValue: "Hủy" })}
          </RippleButton>
          <LoadingButton
            isLoading={isAssigning}
            onClick={() => void handleAssign()}
            variant={mode === "remove" ? "destructive" : "default"}
            className="text-xs font-medium"
          >
            {mode === "remove"
              ? t("proxies.removeButton", { defaultValue: "Gỡ Proxy" })
              : t("proxyAssignment.assignButton", { defaultValue: "Áp dụng" })}
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
