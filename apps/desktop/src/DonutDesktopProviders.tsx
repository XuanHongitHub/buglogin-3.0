import { MotionConfig } from "motion/react";
import { OnbordaProvider } from "onborda";
import { type ReactNode, useEffect } from "react";
import { I18nextProvider } from "react-i18next";
import { Toaster } from "../../../src/components/ui/sonner.js";
import { TooltipProvider } from "../../../src/components/ui/tooltip.js";
import i18n from "../../../src/i18n/index.js";

export function DonutDesktopProviders({ children }: { children: ReactNode }) {
  useEffect(() => {
    document.documentElement.classList.add("dark");
    document.documentElement.style.setProperty(
      "--font-geist-sans",
      "Geist Sans, Segoe UI, sans-serif",
    );
    document.documentElement.style.setProperty(
      "--font-geist-mono",
      "Geist Mono, Cascadia Mono, monospace",
    );
  }, []);

  return (
    <I18nextProvider i18n={i18n}>
      <MotionConfig reducedMotion="user">
        {/* Onborda's renderer imports next/navigation and crashes in the Vite/Tauri
            desktop runtime. Keep its context for Donut screens, but omit that
            Next.js-only renderer on desktop. */}
        <TooltipProvider>
          <OnbordaProvider>{children}</OnbordaProvider>
        </TooltipProvider>
        <Toaster />
      </MotionConfig>
    </I18nextProvider>
  );
}
