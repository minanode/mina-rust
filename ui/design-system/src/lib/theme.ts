/** Theme switching shared by the site and the dashboard. Dark is the default
 * (`class="dark"` on <html>); a stored "light" preference swaps the class. */

export type Theme = "dark" | "light";

export const THEME_STORAGE_KEY = "minanode-theme";

/** Inline script for <head>: applies a stored light preference before the
 * first paint. With nothing stored it does nothing. */
export const themeInitScript = `(function(){try{if(localStorage.getItem("${THEME_STORAGE_KEY}")==="light"){var d=document.documentElement;d.classList.remove("dark");d.classList.add("light");}}catch(e){}})();`;

export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  root.classList.toggle("dark", theme === "dark");
  root.classList.toggle("light", theme === "light");
  try {
    localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Storage can be unavailable (private mode); the switch still works.
  }
}
