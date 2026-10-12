// Runs in <head>, before React: the same choice theme.ts makes, so the first paint is already right.
export const THEME_KEY = "ss-theme";
// It also follows the system while no choice is stored, on every route.
export const THEME_SCRIPT = `(function(){var m=matchMedia("(prefers-color-scheme: dark)"),d=document.documentElement;function a(){try{var t=localStorage.getItem("${THEME_KEY}")}catch(e){}d.dataset.theme=t==="light"||t==="dark"?t:m.matches?"dark":"light";d.dataset.accent="blue"}a();m.addEventListener("change",a)})()`;
