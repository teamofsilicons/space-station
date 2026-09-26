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
const apply = (t: Theme) => (t === "system" ? delete document.documentElement.dataset.theme : (document.documentElement.dataset.theme = t));
const [theme, set] = createSignal<Theme>(read());
apply(theme());

export { theme };
export function setTheme(t: Theme) {
  set(t);
  apply(t);
  try {
    localStorage.setItem("ss-theme", t);
  } catch {}
}
