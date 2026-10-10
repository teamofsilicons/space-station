// The colour scheme: the system's, unless this browser chose one. The choice is `data-theme` on <html>.
import { createSignal } from "solid-js";

export type Theme = "system" | "light" | "dark";
const read = (): Theme => {
  try {
    const t = localStorage.getItem("ss-theme");
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
};
const system = matchMedia("(prefers-color-scheme: dark)");
const apply = (t: Theme) => {
  document.documentElement.dataset.theme = t === "system" ? (system.matches ? "dark" : "light") : t;
  document.documentElement.dataset.accent = "blue";
};
const [theme, set] = createSignal<Theme>(read());
apply(theme());
system.addEventListener("change", () => { if (theme() === "system") apply("system"); });

export { theme };
export function setTheme(t: Theme) {
  set(t);
  apply(t);
  try {
    localStorage.setItem("ss-theme", t);
  } catch {}
}
