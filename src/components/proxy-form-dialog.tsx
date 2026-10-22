"use client";

import { invoke } from "@tauri-apps/api/core";
import {
  type ClipboardEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { LuLoaderCircle } from "react-icons/lu";
import { toast } from "sonner";
import { LoadingButton } from "@/components/loading-button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { translateBackendError } from "@/lib/backend-errors";
import { cn } from "@/lib/utils";
import type { ProxyCheckResult, StoredProxy } from "@/types";
import { RippleButton } from "./ui/ripple";

export interface ProxySettingsDraft {
  proxy_type: string;
  host: string;
  port: number;
  username: string;
  password: string;
}

interface ProxyFormDialogProps {
  isOpen: boolean;
  onClose: () => void;
  editingProxy?: StoredProxy | null;
  embedded?: boolean;
  embeddedFlat?: boolean;
  onCreated?: (proxyId: string) => void;
  onDraftChange?: (settings?: ProxySettingsDraft) => void;
  onNoProxy?: () => void;
  onUpdated?: () => void;
}

const DEFAULT_FORM: ProxySettingsDraft = {
  proxy_type: "http",
  host: "",
  port: 0,
  username: "",
  password: "",
};

export function ProxyFormDialog({
  isOpen,
  onClose,
  editingProxy,
  embedded = false,
  embeddedFlat = false,
  onCreated,
  onDraftChange,
  onNoProxy,
  onUpdated,
}: ProxyFormDialogProps) {
  const { t } = useTranslation();
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [isChecking, setIsChecking] = useState(false);
  const [checkResult, setCheckResult] = useState<ProxyCheckResult | null>(null);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [form, setForm] = useState<ProxySettingsDraft>(() => ({
    ...DEFAULT_FORM,
    proxy_type: embeddedFlat ? "none" : "http",
  }));
  const proxyCheckDescription = useCallback(
    (result: ProxyCheckResult) => {
      const location =
        [result.city, result.region, result.country_code ?? result.country]
          .filter(Boolean)
          .join(" / ") || t("proxyCheck.unknownLocation");
      const coordinates =
        result.latitude != null && result.longitude != null
          ? `${result.latitude.toFixed(6)}, ${result.longitude.toFixed(6)}`
          : t("proxyCheck.unknownLocation");
      return [
        t("proxyCheck.infoIp", { ip: result.ip }),
        t("proxyCheck.infoLocation", { location }),
        t("proxyCheck.infoCoordinates", { coordinates }),
        t("proxyCheck.infoZip", {
          zip: result.zip || t("proxyCheck.unknownLocation"),
        }),
      ].join("\n");
    },
    [t],
  );
  const pendingCreateRef = useRef<{
    fingerprint: string;
    idempotencyKey: string;
    proxyId: string;
    createdAt: string;
  } | null>(null);

  const resetForm = useCallback(() => {
    setForm({ ...DEFAULT_FORM, proxy_type: embeddedFlat ? "none" : "http" });
  }, [embeddedFlat]);

  useEffect(() => {
    if (!isOpen) {
      return;
    }

    if (!editingProxy) {
      resetForm();
      return;
    }

    setForm({
      proxy_type: editingProxy.proxy_settings.proxy_type,
      host: editingProxy.proxy_settings.host,
      port: editingProxy.proxy_settings.port,
      username: editingProxy.proxy_settings.username ?? "",
      password: editingProxy.proxy_settings.password ?? "",
    });
  }, [editingProxy, isOpen, resetForm]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: discard a check result whenever any proxy input changes.
  useEffect(() => {
    setCheckResult(null);
    setCheckError(null);
  }, [form]);

  useEffect(() => {
    if (!embeddedFlat) return;
    if (
      form.proxy_type !== "none" &&
      form.host.trim() &&
      form.port > 0 &&
      form.port <= 65535
    ) {
      onDraftChange?.({ ...form, host: form.host.trim() });
    } else {
      onDraftChange?.(undefined);
    }
  }, [embeddedFlat, form, onDraftChange]);

  const handleSubmit = useCallback(async () => {
    if (!form.host.trim() || !form.port) {
      toast.error(t("proxies.form.hostPortRequired"));
      return;
    }

    setIsSubmitting(true);
    try {
      const payload = {
        name: "",
        proxySettings: {
          proxy_type: form.proxy_type,
          host: form.host.trim(),
          port: form.port,
          username: form.username.trim() || undefined,
          password: form.password.trim() || undefined,
        },
      };

      if (editingProxy) {
        await invoke("update_stored_proxy", {
          proxyId: editingProxy.id,
          ...payload,
        });
        onUpdated?.();
        toast.success(t("toasts.success.proxyUpdated"));
      } else {
        const fingerprint = await fingerprintProxyPayload(payload);
        let pending = pendingCreateRef.current;
        if (!pending || pending.fingerprint !== fingerprint) {
          pending = {
            fingerprint,
            idempotencyKey: crypto.randomUUID(),
            proxyId: `proxy_${crypto.randomUUID()}`,
            createdAt: new Date().toISOString(),
          };
          pendingCreateRef.current = pending;
        }
        await invoke("create_stored_proxy", {
          ...payload,
          idempotencyKey: pending.idempotencyKey,
          proxyId: pending.proxyId,
          createdAt: pending.createdAt,
        });
        pendingCreateRef.current = null;
        onCreated?.(pending.proxyId);
        toast.success(t("toasts.success.proxyCreated"));
      }

      if (!embedded || editingProxy) onClose();
    } catch (error) {
      console.error("Failed to save proxy:", error);
      toast.error(
        t("proxies.form.saveFailed", {
          error: translateBackendError(t, error),
        }),
      );
    } finally {
      setIsSubmitting(false);
    }
  }, [editingProxy, embedded, form, onClose, onCreated, onUpdated, t]);

  const handleClose = useCallback(() => {
    if (!isSubmitting) {
      onClose();
    }
  }, [isSubmitting, onClose]);

  const checkProxySettings = useCallback(
    async (settings: ProxySettingsDraft) => {
      const directNetwork = settings.proxy_type === "none";
      if (
        !directNetwork &&
        (!settings.host.trim() || settings.port < 1 || settings.port > 65535)
      )
        return;
      setCheckResult(null);
      setCheckError(null);
      setIsChecking(true);
      try {
        const proxySettings = directNetwork
          ? { proxy_type: "none" }
          : {
              proxy_type: settings.proxy_type,
              host: settings.host.trim(),
              port: settings.port,
              username: settings.username.trim() || undefined,
              password: settings.password.trim() || undefined,
            };
        const result = await invoke<ProxyCheckResult>("check_proxy_validity", {
          proxySettings,
        });
        if (!result.is_valid) {
          const failMsg = t("proxyCheck.tooltipFailedTitle");
          setCheckError(failMsg);
          toast.error(
            t(
              directNetwork
                ? "createProfile.proxy.networkCheckFailed"
                : "proxyCheck.failed",
              {
                error: failMsg,
              },
            ),
          );
          return;
        }
        setCheckResult(result);
        setCheckError(null);
        toast.success(t("proxyCheck.successTitle"), {
          description: proxyCheckDescription(result),
        });
      } catch (error) {
        const errMsg = translateBackendError(t, error);
        setCheckError(errMsg);
        toast.error(
          t(
            directNetwork
              ? "createProfile.proxy.networkCheckFailed"
              : "proxyCheck.failed",
            {
              error: errMsg,
            },
          ),
        );
      } finally {
        setIsChecking(false);
      }
    },
    [proxyCheckDescription, t],
  );

  const handleHostPaste = useCallback(
    async (event: ClipboardEvent<HTMLInputElement>) => {
      const pasted = event.clipboardData.getData("text").trim();
      if (!pasted.includes(":")) return;
      event.preventDefault();
      try {
        const [parsed] = await invoke<
          Array<{
            status: string;
            proxy_type?: string;
            host?: string;
            port?: number;
            username?: string;
            password?: string;
          }>
        >("parse_txt_proxies", { content: pasted });
        if (
          parsed?.status === "parsed" &&
          parsed.host &&
          parsed.port &&
          ["http", "https", "socks4", "socks5"].includes(
            parsed.proxy_type ?? "",
          )
        ) {
          setForm((current) => ({
            ...current,
            proxy_type: parsed.proxy_type ?? current.proxy_type,
            host: parsed.host ?? "",
            port: parsed.port ?? 0,
            username: parsed.username ?? "",
            password: parsed.password ?? "",
          }));
          void checkProxySettings({
            proxy_type: parsed.proxy_type ?? "http",
            host: parsed.host,
            port: parsed.port,
            username: parsed.username ?? "",
            password: parsed.password ?? "",
          });
          return;
        }
      } catch (error) {
        console.error("Could not parse pasted proxy:", error);
      }
      setForm((current) => ({ ...current, host: pasted }));
    },
    [checkProxySettings],
  );

  const isFormValid = form.host.trim() && form.port > 0 && form.port <= 65535;

  const handleCheckProxy = useCallback(
    () => checkProxySettings(form),
    [checkProxySettings, form],
  );
  const handleCheckNetwork = useCallback(
    () =>
      checkProxySettings({
        proxy_type: "none",
        host: "",
        port: 0,
        username: "",
        password: "",
      }),
    [checkProxySettings],
  );

  const fields = (
    <div
      className={
        embeddedFlat
          ? "@container grid w-full min-w-0 gap-x-4 gap-y-2 py-1 @md:grid-cols-[minmax(0,1fr)_180px]"
          : "@container grid gap-4 py-4"
      }
    >
      <div
        className={
          embeddedFlat
            ? "grid min-w-0 gap-2 @sm:grid-cols-[5.5rem_minmax(0,1fr)] @sm:items-center @md:col-start-1 @md:row-start-1"
            : "grid gap-2"
        }
      >
        <Label
          className={
            embeddedFlat
              ? "font-mono text-xs tracking-tight @sm:text-right"
              : undefined
          }
        >
          {t("proxies.form.type")}
        </Label>
        <div
          className={
            embeddedFlat
              ? "flex min-w-0 flex-nowrap items-center gap-2"
              : undefined
          }
        >
          <Select
            value={form.proxy_type}
            onValueChange={(value) => {
              setForm(
                value === "none"
                  ? { ...DEFAULT_FORM, proxy_type: "none" }
                  : { ...form, proxy_type: value },
              );
              if (value === "none") {
                onDraftChange?.(undefined);
                onNoProxy?.();
              }
            }}
            disabled={isSubmitting}
          >
            <SelectTrigger
              className={
                embeddedFlat
                  ? cn(
                      "font-mono text-xs tracking-tight h-8",
                      form.proxy_type === "none" ? "w-full max-w-72" : "w-32",
                    )
                  : undefined
              }
            >
              <SelectValue placeholder={t("proxies.form.selectType")} />
            </SelectTrigger>
            <SelectContent>
              {embeddedFlat && (
                <SelectItem value="none">
                  {t("createProfile.proxy.noProxyLocal")}
                </SelectItem>
              )}
              {["http", "https", "socks4", "socks5"].map((type) => (
                <SelectItem key={type} value={type}>
                  {type.toUpperCase()}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      {embeddedFlat ? (
        form.proxy_type !== "none" && (
          <div className="grid min-w-0 gap-2 @sm:grid-cols-[5.5rem_minmax(0,1fr)] @sm:items-center @md:col-start-1 @md:row-start-2">
            <Label
              htmlFor="proxy-host"
              className="font-mono text-xs tracking-tight @sm:text-right"
            >
              {t("proxies.form.host")}: {t("proxies.form.port")}
            </Label>
            <div className="flex min-w-0 items-center gap-2">
              <Input
                id="proxy-host"
                value={form.host}
                onPaste={(event) => void handleHostPaste(event)}
                onChange={(e) => {
                  setForm({ ...form, host: e.target.value });
                }}
                placeholder={t("proxies.form.hostPlaceholder")}
                disabled={isSubmitting}
                className="min-w-0 flex-1 font-mono text-xs tracking-tight h-8"
              />
              <span
                aria-hidden="true"
                className="text-muted-foreground text-xs"
              >
                :
              </span>
              <Input
                id="proxy-port"
                type="number"
                value={form.port || ""}
                onChange={(e) => {
                  setForm({
                    ...form,
                    port: Number.parseInt(e.target.value, 10) || 0,
                  });
                }}
                placeholder={t("proxies.form.portPlaceholder")}
                min="1"
                max="65535"
                disabled={isSubmitting}
                className="w-20 shrink-0 font-mono text-xs tracking-tight h-8"
                aria-label={t("proxies.form.port")}
              />
            </div>
          </div>
        )
      ) : (
        <div className="grid grid-cols-2 gap-4">
          <div className="grid gap-2">
            <Label htmlFor="proxy-host">{t("proxies.form.host")}</Label>
            <Input
              id="proxy-host"
              value={form.host}
              onPaste={(event) => void handleHostPaste(event)}
              onChange={(e) => {
                setForm({ ...form, host: e.target.value });
              }}
              placeholder={t("proxies.form.hostPlaceholder")}
              disabled={isSubmitting}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="proxy-port">{t("proxies.form.port")}</Label>
            <Input
              id="proxy-port"
              type="number"
              value={form.port || ""}
              onChange={(e) => {
                setForm({
                  ...form,
                  port: Number.parseInt(e.target.value, 10) || 0,
                });
              }}
              placeholder={t("proxies.form.portPlaceholder")}
              min="1"
              max="65535"
              disabled={isSubmitting}
            />
          </div>
        </div>
      )}

      {(!embeddedFlat || form.proxy_type !== "none") && (
        <div
          className={
            embeddedFlat
              ? "grid gap-2 @md:col-start-1 @md:row-start-3"
              : "grid grid-cols-1 gap-4 @sm:grid-cols-2"
          }
        >
          <div
            className={
              embeddedFlat
                ? "grid min-w-0 gap-2 @sm:grid-cols-[5.5rem_minmax(0,1fr)] @sm:items-center"
                : "grid gap-2"
            }
          >
            <Label
              htmlFor="proxy-username"
              className={
                embeddedFlat
                  ? "font-mono text-xs tracking-tight @sm:text-right"
                  : undefined
              }
            >
              {t("proxies.form.username")}
            </Label>
            <Input
              id="proxy-username"
              value={form.username}
              onChange={(e) => {
                setForm({ ...form, username: e.target.value });
              }}
              placeholder={t("proxies.form.usernamePlaceholder")}
              disabled={isSubmitting}
              className={
                embeddedFlat
                  ? "font-mono text-xs tracking-tight h-8"
                  : undefined
              }
            />
          </div>

          <div
            className={
              embeddedFlat
                ? "grid min-w-0 gap-2 @sm:grid-cols-[5.5rem_minmax(0,1fr)] @sm:items-center"
                : "grid gap-2"
            }
          >
            <Label
              htmlFor="proxy-password"
              className={
                embeddedFlat
                  ? "font-mono text-xs tracking-tight @sm:text-right"
                  : undefined
              }
            >
              {t("proxies.form.password")}
            </Label>
            <Input
              id="proxy-password"
              type="password"
              value={form.password}
              onChange={(e) => {
                setForm({ ...form, password: e.target.value });
              }}
              placeholder={t("proxies.form.passwordPlaceholder")}
              disabled={isSubmitting}
              className={
                embeddedFlat
                  ? "font-mono text-xs tracking-tight h-8"
                  : undefined
              }
            />
          </div>
        </div>
      )}
      {embeddedFlat && form.proxy_type !== "none" && (
        <aside className="flex min-w-0 flex-col items-stretch gap-2 @md:col-start-2 @md:row-start-2 @md:row-span-2 @md:border-l @md:border-border/40 @md:pl-4">
          <RippleButton
            variant="outline"
            size="sm"
            onClick={() => void handleCheckProxy()}
            disabled={!isFormValid || isChecking || isSubmitting}
            className="h-8 w-full justify-center gap-1.5 text-xs font-mono tracking-tight shrink-0"
          >
            {isChecking && (
              <LuLoaderCircle className="size-3.5 animate-spin text-primary" />
            )}
            {isChecking
              ? t("common.checking", { defaultValue: "Đang kiểm tra..." })
              : t("proxies.form.checkProxy")}
          </RippleButton>
          <div className="h-[72px] w-full" aria-live="polite">
            {isChecking ? (
              <div className="flex h-full flex-col items-center justify-center rounded-lg border border-primary/20 bg-primary/5 p-2 text-center text-xs text-primary animate-pulse">
                <LuLoaderCircle className="size-4 animate-spin mb-1 text-primary" />
                <span className="text-[11px] font-medium">
                  {t("common.checking", { defaultValue: "Đang kiểm tra..." })}
                </span>
              </div>
            ) : checkError ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <div
                    role="status"
                    className="flex h-full flex-col justify-center rounded-lg border border-destructive/30 bg-destructive/10 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-destructive/15 cursor-help"
                  >
                    <div className="flex items-center gap-1.5 text-destructive font-semibold text-xs">
                      <span className="size-2 shrink-0 rounded-full bg-destructive" />
                      <span className="truncate">
                        {t("proxyCheck.tooltipFailedTitle", {
                          defaultValue: "Lỗi kết nối",
                        })}
                      </span>
                    </div>
                    <div
                      className="mt-1 truncate font-mono text-[11px] text-destructive/80"
                      title={checkError}
                    >
                      {checkError}
                    </div>
                  </div>
                </TooltipTrigger>
                <TooltipContent
                  side="top"
                  className="max-w-xs text-xs whitespace-pre-line font-mono"
                >
                  {checkError}
                </TooltipContent>
              </Tooltip>
            ) : checkResult ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <div
                    role="status"
                    className="flex h-full flex-col justify-center rounded-lg border border-emerald-500/25 bg-emerald-500/10 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-emerald-500/15 cursor-help"
                  >
                    <div className="flex items-center justify-between gap-1">
                      <div className="flex items-center gap-1.5 min-w-0">
                        <span className="size-2 shrink-0 rounded-full bg-emerald-400 animate-pulse" />
                        <span className="font-mono text-xs font-semibold text-emerald-300 truncate">
                          {checkResult.ip}
                        </span>
                      </div>
                      {checkResult.latency_ms != null && (
                        <span className="font-mono text-[10px] text-emerald-400/80 shrink-0">
                          {checkResult.latency_ms}ms
                        </span>
                      )}
                    </div>
                    <div className="mt-1 truncate text-[11px] font-medium text-emerald-200/90">
                      {[
                        checkResult.city,
                        checkResult.region,
                        checkResult.country_code ?? checkResult.country,
                      ]
                        .filter(Boolean)
                        .join(", ") || t("proxyCheck.unknownLocation")}
                    </div>
                    {(checkResult.latitude != null || checkResult.zip) && (
                      <div className="mt-0.5 truncate font-mono text-[10px] text-emerald-300/60">
                        {checkResult.latitude != null
                          ? `${checkResult.latitude.toFixed(2)}, ${checkResult.longitude?.toFixed(2)}`
                          : ""}
                        {checkResult.zip ? ` · ${checkResult.zip}` : ""}
                      </div>
                    )}
                  </div>
                </TooltipTrigger>
                <TooltipContent
                  side="top"
                  className="max-w-xs text-xs whitespace-pre-line font-mono"
                >
                  {proxyCheckDescription(checkResult)}
                </TooltipContent>
              </Tooltip>
            ) : (
              <div className="flex h-full flex-col items-center justify-center rounded-lg border border-dashed border-border/50 bg-muted/20 px-2 text-center text-[11px] text-muted-foreground/60">
                <span>
                  {t("createProfile.proxy.readyToCheck", {
                    defaultValue: "Chưa kiểm tra proxy",
                  })}
                </span>
              </div>
            )}
          </div>
        </aside>
      )}
      {embeddedFlat && form.proxy_type === "none" && (
        <aside
          className="flex min-w-0 flex-col items-stretch gap-2 @md:col-start-2 @md:row-start-1 @md:row-span-3 @md:border-l @md:border-border/40 @md:pl-4"
          aria-live="polite"
        >
          <RippleButton
            variant="outline"
            size="sm"
            onClick={() => void handleCheckNetwork()}
            disabled={isChecking || isSubmitting}
            className="h-8 w-full justify-center gap-1.5 text-xs font-mono tracking-tight shrink-0"
          >
            {isChecking && (
              <LuLoaderCircle className="size-3.5 animate-spin text-primary" />
            )}
            {isChecking
              ? t("common.checking", { defaultValue: "Đang kiểm tra..." })
              : t("createProfile.proxy.checkNetwork")}
          </RippleButton>
          <div className="h-[72px] w-full">
            {isChecking ? (
              <div className="flex h-full flex-col items-center justify-center rounded-lg border border-primary/20 bg-primary/5 p-2 text-center text-xs text-primary animate-pulse">
                <LuLoaderCircle className="size-4 animate-spin mb-1 text-primary" />
                <span className="text-[11px] font-medium">
                  {t("common.checking", { defaultValue: "Đang kiểm tra..." })}
                </span>
              </div>
            ) : checkError ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <div
                    role="status"
                    className="flex h-full flex-col justify-center rounded-lg border border-destructive/30 bg-destructive/10 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-destructive/15 cursor-help"
                  >
                    <div className="flex items-center gap-1.5 text-destructive font-semibold text-xs">
                      <span className="size-2 shrink-0 rounded-full bg-destructive" />
                      <span className="truncate">
                        {t("proxyCheck.tooltipFailedTitle", {
                          defaultValue: "Lỗi kết nối",
                        })}
                      </span>
                    </div>
                    <div
                      className="mt-1 truncate font-mono text-[11px] text-destructive/80"
                      title={checkError}
                    >
                      {checkError}
                    </div>
                  </div>
                </TooltipTrigger>
                <TooltipContent
                  side="top"
                  className="max-w-xs text-xs whitespace-pre-line font-mono"
                >
                  {checkError}
                </TooltipContent>
              </Tooltip>
            ) : checkResult ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <div
                    role="status"
                    className="flex h-full flex-col justify-center rounded-lg border border-emerald-500/25 bg-emerald-500/10 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-emerald-500/15 cursor-help"
                  >
                    <div className="flex items-center justify-between gap-1">
                      <div className="flex items-center gap-1.5 min-w-0">
                        <span className="size-2 shrink-0 rounded-full bg-emerald-400 animate-pulse" />
                        <span className="font-mono text-xs font-semibold text-emerald-300 truncate">
                          {checkResult.ip}
                        </span>
                      </div>
                      {checkResult.latency_ms != null && (
                        <span className="font-mono text-[10px] text-emerald-400/80 shrink-0">
                          {checkResult.latency_ms}ms
                        </span>
                      )}
                    </div>
                    <div className="mt-1 truncate text-[11px] font-medium text-emerald-200/90">
                      {[
                        checkResult.city,
                        checkResult.region,
                        checkResult.country_code ?? checkResult.country,
                      ]
                        .filter(Boolean)
                        .join(", ") || t("proxyCheck.unknownLocation")}
                    </div>
                    {(checkResult.latitude != null || checkResult.zip) && (
                      <div className="mt-0.5 truncate font-mono text-[10px] text-emerald-300/60">
                        {checkResult.latitude != null
                          ? `${checkResult.latitude.toFixed(2)}, ${checkResult.longitude?.toFixed(2)}`
                          : ""}
                        {checkResult.zip ? ` · ${checkResult.zip}` : ""}
                      </div>
                    )}
                  </div>
                </TooltipTrigger>
                <TooltipContent
                  side="top"
                  className="max-w-xs text-xs whitespace-pre-line font-mono"
                >
                  {proxyCheckDescription(checkResult)}
                </TooltipContent>
              </Tooltip>
            ) : (
              <div className="flex h-full flex-col items-center justify-center rounded-lg border border-dashed border-border/50 bg-muted/20 px-2 text-center text-[11px] text-muted-foreground/60">
                <span>
                  {t("createProfile.proxy.readyToCheckNetwork", {
                    defaultValue: "Chưa kiểm tra mạng",
                  })}
                </span>
              </div>
            )}
          </div>
        </aside>
      )}
    </div>
  );

  const footer = (
    <DialogFooter>
      {!(embedded && embeddedFlat) && (
        <RippleButton
          variant="outline"
          onClick={handleClose}
          disabled={isSubmitting}
        >
          {t("common.buttons.cancel")}
        </RippleButton>
      )}
      <LoadingButton
        isLoading={isSubmitting}
        onClick={handleSubmit}
        disabled={!isFormValid}
      >
        {editingProxy ? t("proxies.edit") : t("proxies.add")}
      </LoadingButton>
    </DialogFooter>
  );

  if (embeddedFlat) {
    return <div className="contents">{fields}</div>;
  }

  if (!isOpen) return null;
  if (embedded) {
    return (
      <section className="rounded-xl border border-foreground/10 bg-foreground/[0.025] p-4">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-sm font-semibold">{t("proxies.add")}</h3>
          <RippleButton
            variant="ghost"
            size="sm"
            onClick={handleClose}
            disabled={isSubmitting}
          >
            {t("common.buttons.cancel")}
          </RippleButton>
        </div>
        {fields}
        {footer}
      </section>
    );
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            {editingProxy ? t("proxies.edit") : t("proxies.add")}
          </DialogTitle>
        </DialogHeader>
        {fields}
        {footer}
      </DialogContent>
    </Dialog>
  );
}

async function fingerprintProxyPayload(payload: unknown): Promise<string> {
  const bytes = new TextEncoder().encode(JSON.stringify(payload));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}
