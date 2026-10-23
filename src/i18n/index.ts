import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import en from "./locales/en.json";
import vi from "./locales/vi.json";

export const SUPPORTED_LANGUAGES = [
  { code: "vi", name: "Vietnamese", nativeName: "Tiếng Việt" },
  { code: "en", name: "English", nativeName: "English" },
] as const;

export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number]["code"];

const LANGUAGE_FALLBACKS: Record<string, string[]> = {
  "vi-VN": ["vi", "en"],
  "en-US": ["en", "vi"],
  "en-GB": ["en", "vi"],
};

export function getLanguageWithFallback(systemLocale: string): string {
  const baseLanguage = systemLocale.split(/[-_]/)[0].toLowerCase();

  if (SUPPORTED_LANGUAGES.some((lang) => lang.code === baseLanguage)) {
    return baseLanguage;
  }

  if (LANGUAGE_FALLBACKS[systemLocale]) {
    return LANGUAGE_FALLBACKS[systemLocale][0];
  }

  if (LANGUAGE_FALLBACKS[baseLanguage]) {
    return LANGUAGE_FALLBACKS[baseLanguage][0];
  }

  return "vi";
}

const resources = {
  vi: { translation: vi },
  en: { translation: en },
};

i18n.use(initReactI18next).init({
  resources,
  lng: "vi",
  fallbackLng: "vi",
  interpolation: {
    escapeValue: false,
  },
  react: {
    useSuspense: false,
  },
});

export default i18n;
