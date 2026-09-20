import { create } from "zustand";
import en from "./en";
import ptBR from "./ptBR";

export type Lang = "en" | "pt-BR";

const dictionaries: Record<Lang, unknown> = { en, "pt-BR": ptBR };

function detect(): Lang {
  const nav = typeof navigator !== "undefined" ? navigator.language : "en";
  return nav.toLowerCase().startsWith("pt") ? "pt-BR" : "en";
}

interface LangState {
  lang: Lang;
  setLang: (l: Lang) => void;
}

export const useLang = create<LangState>((set) => ({
  lang: detect(),
  setLang: (lang) => {
    document.documentElement.lang = lang;
    set({ lang });
  },
}));

export function currentLocale(): string {
  return useLang.getState().lang;
}

function lookup(dict: unknown, key: string): string | undefined {
  let cur: unknown = dict;
  for (const part of key.split(".")) {
    if (cur && typeof cur === "object" && part in (cur as Record<string, unknown>)) {
      cur = (cur as Record<string, unknown>)[part];
    } else {
      return undefined;
    }
  }
  return typeof cur === "string" ? cur : undefined;
}

export type TParams = Record<string, string | number>;

/** Translate a dotted key; `{name}` placeholders are replaced from params. */
export function translate(lang: Lang, key: string, params?: TParams): string {
  const s = lookup(dictionaries[lang], key) ?? lookup(en, key) ?? key;
  if (!params) return s;
  return s.replace(/\{(\w+)\}/g, (_, k) => (k in params ? String(params[k]) : `{${k}}`));
}

export function t(key: string, params?: TParams): string {
  return translate(useLang.getState().lang, key, params);
}

/** Hook that re-renders the component when the language changes. */
export function useT() {
  const lang = useLang((s) => s.lang);
  return (key: string, params?: TParams) => translate(lang, key, params);
}

/** Has a translation for this key (used for optional reason/kind keys). */
export function hasKey(key: string): boolean {
  return lookup(en, key) !== undefined;
}
