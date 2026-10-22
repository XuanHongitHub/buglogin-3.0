"use client";

import { invoke } from "@tauri-apps/api/core";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { FiCheck } from "react-icons/fi";
import { toast } from "sonner";
import { FlagIcon } from "@/components/flag-icon";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { extractRootError } from "@/lib/error-utils";
import { formatRelativeTime } from "@/lib/flag-utils";
import { MOTION_EASE_OUT } from "@/lib/motion";
import type { ProxyCheckResult, StoredProxy } from "@/types";

interface ProxyCheckButtonProps {
  proxy: StoredProxy;
  profileId: string;
  checkingProfileId: string | null;
  cachedResult?: ProxyCheckResult;
  onCheckComplete?: (result: ProxyCheckResult) => void;
  onCheckFailed?: (result: ProxyCheckResult) => void;
  disabled?: boolean;
  setCheckingProfileId?: (id: string | null) => void;
}

export function ProxyCheckButton({
  proxy,
  profileId,
  checkingProfileId,
  cachedResult,
  onCheckComplete,
  onCheckFailed,
  disabled = false,
  setCheckingProfileId,
}: ProxyCheckButtonProps) {
  const { t } = useTranslation();
  const isBugLoginDesktop =
    process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "desktop";
  const reduceMotion = useReducedMotion();
  const [localResult, setLocalResult] = React.useState<
    ProxyCheckResult | undefined
  >(cachedResult);

  React.useEffect(() => {
    setLocalResult(cachedResult);
  }, [cachedResult]);

  const handleCheck = React.useCallback(async () => {
    if (checkingProfileId === profileId) return;

    setCheckingProfileId?.(profileId);
    try {
      const result: ProxyCheckResult = isBugLoginDesktop
        ? await invoke<{ status: string; latencyMs: number }>(
            "check_stored_proxy",
            { proxyId: proxy.id },
          ).then((check) => ({
            ip: "",
            timestamp: Math.floor(Date.now() / 1000),
            is_valid: check.status === "healthy",
            latency_ms: check.latencyMs,
          }))
        : await invoke<ProxyCheckResult>("check_proxy_validity", {
            proxyId: proxy.id,
            proxySettings: proxy.proxy_settings,
          });
      setLocalResult(result);
      onCheckComplete?.(result);

      if (isBugLoginDesktop) {
        toast.success(
          t("proxyCheck.connected", { latency: result.latency_ms ?? 0 }),
        );
        return;
      }

      // Show toast with location
      const locationParts: string[] = [];
      if (result.city) locationParts.push(result.city);
      if (result.country) locationParts.push(result.country);
      const location =
        locationParts.length > 0
          ? locationParts.join(", ")
          : t("proxyCheck.unknownLocation");

      toast.success(
        <div className="flex flex-col">
          {t("proxyCheck.locationToast")}
          <div className="flex items-center whitespace-nowrap">
            {location}
            {result.country_code && (
              <FlagIcon
                countryCode={result.country_code}
                className="ml-1 text-sm"
              />
            )}
          </div>
        </div>,
      );
    } catch (error) {
      const errorMessage = extractRootError(error);
      toast.error(t("proxyCheck.failed", { error: errorMessage }));

      // Save failed check result
      const failedResult: ProxyCheckResult = {
        ip: "",
        city: undefined,
        country: undefined,
        country_code: undefined,
        timestamp: Math.floor(Date.now() / 1000),
        is_valid: false,
      };
      setLocalResult(failedResult);
      onCheckFailed?.(failedResult);
    } finally {
      setCheckingProfileId?.(null);
    }
  }, [
    proxy,
    profileId,
    checkingProfileId,
    onCheckComplete,
    onCheckFailed,
    setCheckingProfileId,
    t,
    isBugLoginDesktop,
  ]);

  const isCurrentlyChecking = checkingProfileId === profileId;
  const result = localResult;
  const statusKey = isCurrentlyChecking
    ? "checking"
    : result?.is_valid
      ? "valid-location"
      : result && !result.is_valid
        ? "invalid"
        : "idle";

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="size-7 p-0"
          onClick={handleCheck}
          disabled={isCurrentlyChecking || disabled}
        >
          <AnimatePresence initial={false} mode="wait">
            <motion.span
              key={statusKey}
              initial={{ opacity: 0, scale: reduceMotion ? 1 : 0.9 }}
              animate={{
                opacity: 1,
                scale: 1,
                transition: {
                  duration: reduceMotion ? 0.15 : 0.16,
                  ease: MOTION_EASE_OUT,
                },
              }}
              exit={{
                opacity: 0,
                scale: reduceMotion ? 1 : 0.9,
                transition: {
                  duration: reduceMotion ? 0.15 : 0.1,
                  ease: MOTION_EASE_OUT,
                },
              }}
              className="inline-flex size-3 items-center justify-center"
            >
              {isCurrentlyChecking ? (
                <span className="size-3 animate-spin rounded-full border border-current border-t-transparent" />
              ) : result?.is_valid ? (
                <span className="relative inline-flex items-center justify-center">
                  {!isBugLoginDesktop && (
                    <FlagIcon
                      countryCode={result.country_code}
                      className="h-2.5"
                    />
                  )}
                  <FiCheck
                    className={
                      isBugLoginDesktop
                        ? "size-3"
                        : "absolute right-[-4px] bottom-[-6px]"
                    }
                  />
                </span>
              ) : result && !result.is_valid ? (
                <span className="text-sm text-destructive">✕</span>
              ) : (
                <FiCheck className="size-3" />
              )}
            </motion.span>
          </AnimatePresence>
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        {isCurrentlyChecking ? (
          <p>{t("proxyCheck.tooltipChecking")}</p>
        ) : result?.is_valid ? (
          <div className="space-y-1">
            {isBugLoginDesktop ? (
              <p>
                {t("proxyCheck.connected", {
                  latency: result.latency_ms ?? 0,
                })}
              </p>
            ) : (
              <>
                <p className="flex items-center gap-1">
                  {result.country_code && (
                    <FlagIcon countryCode={result.country_code} />
                  )}
                  {[result.city, result.country].filter(Boolean).join(", ") ||
                    t("proxyCheck.unknownLocation")}
                </p>
                <p className="text-xs text-primary-foreground/70">
                  {t("proxyCheck.tooltipIp", { ip: result.ip })}
                </p>
              </>
            )}
            <p className="text-xs text-primary-foreground/70">
              {t("proxyCheck.tooltipChecked", {
                time: formatRelativeTime(result.timestamp),
              })}
            </p>
          </div>
        ) : result && !result.is_valid ? (
          <div>
            <p>{t("proxyCheck.tooltipFailedTitle")}</p>
            <p className="text-xs text-primary-foreground/70">
              {t("proxyCheck.tooltipFailed", {
                time: formatRelativeTime(result.timestamp),
              })}
            </p>
          </div>
        ) : (
          <p>{t("proxyCheck.tooltipDefault")}</p>
        )}
      </TooltipContent>
    </Tooltip>
  );
}
