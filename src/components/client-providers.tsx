"use client";

import { MotionConfig } from "motion/react";
import { useEffect } from "react";
import { I18nProvider } from "@/components/i18n-provider";
import { OnboardingProvider } from "@/components/onboarding-provider";
import { CustomThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { setupLogging } from "@/lib/logger";

export function ClientProviders({ children }: { children: React.ReactNode }) {
  const isWebHost = process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET === "web";

  useEffect(() => {
    if (isWebHost) return;
    void setupLogging();
  }, [isWebHost]);

  if (isWebHost) {
    return (
      <MotionConfig reducedMotion="user">
        <TooltipProvider>{children}</TooltipProvider>
        <Toaster />
      </MotionConfig>
    );
  }

  return (
    <I18nProvider>
      <CustomThemeProvider>
        <MotionConfig reducedMotion="user">
          <TooltipProvider>
            <OnboardingProvider>{children}</OnboardingProvider>
          </TooltipProvider>
          <Toaster />
        </MotionConfig>
      </CustomThemeProvider>
    </I18nProvider>
  );
}
