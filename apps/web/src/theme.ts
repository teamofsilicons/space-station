"use client";
// The colour scheme: the system's, unless this browser chose one. The choice is `data-theme` on <html>;
// THEME_SCRIPT (theme-script.ts) sets it before the first paint and follows the system, `setTheme` the rest.
import { useSyncExternalStore } from "react";
import { THEME_KEY as KEY } from "./theme-script";

export type Theme = "system" | "light" | "dark";

const read = (): Theme => {
  try {
    const t = localStorage.getItem(KEY);
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
};
const apply = (t: Theme) => {
  document.documentElement.dataset.theme = t === "system" ? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light") : t;
  document.documentElement.dataset.accent = "blue";
};

const listeners = new Set<() => void>();
let current: Theme | undefined;
const subscribe = (fn: () => void) => (listeners.add(fn), () => void listeners.delete(fn));

export const useTheme = () => useSyncExternalStore(subscribe, () => (current ??= read()), () => "system" as Theme);

export function setTheme(t: Theme) {
  current = t;
  apply(t);
  try {
    localStorage.setItem(KEY, t);
  } catch {}
  listeners.forEach((fn) => fn());
}
