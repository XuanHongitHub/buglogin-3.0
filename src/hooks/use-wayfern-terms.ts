import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";

interface UseWayfernTermsReturn {
  termsAccepted: boolean | null;
  isLoading: boolean;
  checkTerms: () => Promise<void>;
}

export function useWayfernTerms(): UseWayfernTermsReturn {
  const [termsAccepted, setTermsAccepted] = useState<boolean | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  const checkTerms = useCallback(async () => {
    try {
      const installedVersions = await invoke<string[]>(
        "runtime_installed_versions_get",
        {
          runtime: "wayfern",
          correlationId: `cor-${crypto.randomUUID().replaceAll("-", "")}`,
        },
      );

      // Consent is only needed when the Wayfern runtime can actually be used.
      if (installedVersions.length === 0) {
        setTermsAccepted(true);
      } else {
        setTermsAccepted(await invoke<boolean>("check_wayfern_terms_accepted"));
      }
    } catch (error) {
      console.error("Failed to check terms acceptance:", error);
      // Unknown runtime state must not be presented as a consent request.
      // Keep consent-dependent actions disabled until the runtime can be read.
      setTermsAccepted(null);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void checkTerms();
  }, [checkTerms]);

  return {
    termsAccepted,
    isLoading,
    checkTerms,
  };
}
