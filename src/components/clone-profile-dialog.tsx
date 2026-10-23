"use client";

import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import * as React from "react";
import { useTranslation } from "react-i18next";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { showErrorToast } from "@/lib/toast-utils";
import type { BrowserProfile } from "@/types";
import { createTauriDesktopApi } from "../../apps/desktop/src/bridge";
import { LoadingButton } from "./loading-button";
import { RippleButton } from "./ui/ripple";

interface CloneProfileDialogProps {
  isOpen: boolean;
  onClose: () => void;
  profile: BrowserProfile | null;
  onCloneComplete?: () => void;
}

export function CloneProfileDialog({
  isOpen,
  onClose,
  profile,
  onCloneComplete,
}: CloneProfileDialogProps) {
  const { t } = useTranslation();
  const [name, setName] = React.useState("");
  const [isLoading, setIsLoading] = React.useState(false);
  const inputRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    if (!(isOpen && profile)) {
      setIsLoading(false);
      return;
    }
    setName(`${profile.name} (Copy)`);
    const handle = window.setTimeout(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    }, 0);
    return () => {
      window.clearTimeout(handle);
    };
  }, [isOpen, profile]);

  if (!profile) return null;

  const handleClone = async () => {
    if (!name.trim() || isLoading) return;
    setIsLoading(true);
    try {
      if (process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "desktop") {
        const revision = profile.revision;
        if (!Number.isSafeInteger(revision) || (revision ?? 0) < 1) {
          throw new Error(
            "Profile revision is unavailable. Refresh profiles and retry.",
          );
        }
        await createTauriDesktopApi().cloneProfile({
          sourceProfileId: profile.id,
          targetProfileId: `profile_${crypto.randomUUID()}`,
          name: name.trim(),
          expectedRevision: revision as number,
        });
      } else {
        await invoke<BrowserProfile>("clone_profile", {
          profileId: profile.id,
          name: name.trim(),
        });
      }
      try {
        await emit("profiles-changed", { profileId: profile.id });
      } catch {
        // ignore if not running in tauri context
      }
      onClose();
      onCloneComplete?.();
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : String(err);
      showErrorToast(t("errors.cloneProfileFailed", { error: errorMessage }));
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("profileInfo.clone.title")}</DialogTitle>
          <DialogDescription>
            {t(
              process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "desktop"
                ? "profileInfo.clone.metadataOnlyDescription"
                : "profileInfo.clone.description",
            )}
          </DialogDescription>
        </DialogHeader>
        <Input
          ref={inputRef}
          value={name}
          onChange={(e) => {
            setName(e.target.value);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") void handleClone();
          }}
          placeholder={t("profileInfo.clone.namePlaceholder")}
          disabled={isLoading}
        />
        <DialogFooter>
          <RippleButton
            variant="outline"
            onClick={onClose}
            disabled={isLoading}
          >
            {t("common.buttons.cancel")}
          </RippleButton>
          <LoadingButton
            onClick={() => void handleClone()}
            isLoading={isLoading}
            disabled={!name.trim()}
          >
            {t("profileInfo.clone.button")}
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
