"use client";

import * as React from "react";
import { Moon, Sun } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import {
  applyTheme,
  THEME_STORAGE_KEY,
  type Theme,
} from "@minanode/design-system/lib/theme";
import { cn } from "@minanode/design-system/lib/utils";

/** Icon button that switches between the dark and light themes. Put
 * `themeInitScript` from lib/theme in <head> so a stored choice applies before
 * the first paint. */
function ThemeToggle({ className }: { className?: string }) {
  const [theme, setTheme] = React.useState<Theme>("dark");

  // Follow the page's theme class, whoever sets it: this toggle, another
  // toggle on the page, or another tab through storage.
  React.useEffect(() => {
    const root = document.documentElement;
    const read = () =>
      setTheme(root.classList.contains("light") ? "light" : "dark");
    read();
    const observer = new MutationObserver(read);
    observer.observe(root, { attributes: true, attributeFilter: ["class"] });
    const onStorage = (event: StorageEvent) => {
      if (event.key === THEME_STORAGE_KEY && event.newValue) {
        applyTheme(event.newValue === "light" ? "light" : "dark");
      }
    };
    window.addEventListener("storage", onStorage);
    return () => {
      observer.disconnect();
      window.removeEventListener("storage", onStorage);
    };
  }, []);

  const next: Theme = theme === "light" ? "dark" : "light";
  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      className={cn(
        "size-10 sm:size-8 text-muted-foreground hover:text-foreground",
        className,
      )}
      aria-label={`Switch to ${next} theme`}
      title={`Switch to ${next} theme`}
      onClick={() => {
        applyTheme(next);
        setTheme(next);
      }}
    >
      {theme === "light" ? <Moon /> : <Sun />}
    </Button>
  );
}

export { ThemeToggle };
