"use client";

import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuCheck, LuGlobe } from "react-icons/lu";
import {
  ProxyFormDialog,
  type ProxySettingsDraft,
} from "@/components/proxy-form-dialog";
import {
  AnimatedTabs,
  AnimatedTabsList,
  AnimatedTabsTrigger,
} from "@/components/ui/animated-tabs";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { cn } from "@/lib/utils";
import type { StoredProxy } from "@/types";

interface CreateProfileProxyDialogProps {
  proxies: StoredProxy[];
  selectedProxyId?: string;
  onSelect: (proxyId?: string) => void;
  onDraftChange: (settings?: ProxySettingsDraft) => void;
}

export function CreateProfileProxyDialog({
  proxies,
  selectedProxyId,
  onSelect,
  onDraftChange,
}: CreateProfileProxyDialogProps) {
  const { t } = useTranslation();
  const [tab, setTab] = useState("custom");
  const handleDraftChange = useCallback(
    (settings?: ProxySettingsDraft) => {
      onDraftChange(settings);
    },
    [onDraftChange],
  );

  return (
    <section className="max-w-[48rem] space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2 font-mono text-sm font-semibold tracking-tight">
          <LuGlobe
            className="size-4 text-muted-foreground"
            aria-hidden="true"
          />
          {t("createProfile.proxy.title")}
        </div>
        <AnimatedTabs value={tab} onValueChange={setTab}>
          <AnimatedTabsList className="rounded-lg bg-muted p-1">
            <AnimatedTabsTrigger value="custom">
              {t("createProfile.proxy.customTab")}
            </AnimatedTabsTrigger>
            <AnimatedTabsTrigger value="saved">
              {t("createProfile.proxy.savedTab")}
              <span className="text-xs text-muted-foreground">
                {proxies.length}
              </span>
            </AnimatedTabsTrigger>
          </AnimatedTabsList>
        </AnimatedTabs>
      </div>

      <div hidden={tab !== "custom"}>
        <ProxyFormDialog
          isOpen={tab === "custom"}
          embedded
          embeddedFlat
          onClose={() => setTab("saved")}
          onDraftChange={handleDraftChange}
          onNoProxy={() => onSelect(undefined)}
        />
      </div>

      {tab === "saved" && (
        <Command className="rounded-lg border bg-background">
          <CommandInput placeholder={t("createProfile.proxy.searchSaved")} />
          <CommandList className="max-h-48">
            <CommandEmpty>
              {proxies.length === 0
                ? t("createProfile.proxy.noSavedProxies")
                : t("createProfile.proxy.notFound")}
            </CommandEmpty>
            <CommandItem
              value="no-proxy"
              keywords={[t("createProfile.proxy.noProxy")]}
              onSelect={() => {
                onDraftChange(undefined);
                onSelect(undefined);
              }}
              className="mx-1 my-1"
            >
              <LuGlobe className="size-4 shrink-0" aria-hidden="true" />
              <span className="flex-1">{t("createProfile.proxy.noProxy")}</span>
              <LuCheck
                className={cn(
                  "size-4 shrink-0",
                  !selectedProxyId ? "opacity-100" : "opacity-0",
                )}
                aria-hidden="true"
              />
            </CommandItem>
            {proxies.map((proxy) => (
              <CommandItem
                key={proxy.id}
                value={`${proxy.proxy_settings.host}:${proxy.proxy_settings.port}`}
                keywords={[
                  proxy.proxy_settings.host,
                  String(proxy.proxy_settings.port),
                  proxy.proxy_settings.proxy_type,
                ]}
                onSelect={() => {
                  onDraftChange(undefined);
                  onSelect(proxy.id);
                }}
                className="mx-1"
              >
                <span className="w-14 shrink-0 font-mono text-xs text-muted-foreground">
                  {proxy.proxy_settings.proxy_type.toUpperCase()}
                </span>
                <span className="flex-1 truncate font-mono text-xs tracking-tight">
                  {proxy.proxy_settings.host}:{proxy.proxy_settings.port}
                </span>
                <LuCheck
                  className={cn(
                    "size-4 shrink-0",
                    selectedProxyId === proxy.id ? "opacity-100" : "opacity-0",
                  )}
                  aria-hidden="true"
                />
              </CommandItem>
            ))}
          </CommandList>
        </Command>
      )}
    </section>
  );
}
