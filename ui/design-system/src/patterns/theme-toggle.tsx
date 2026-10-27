"use client";

import * as React from "react";
import { Moon, Sun } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { applyTheme, type Theme } from "@minanode/design-system/lib/theme";
import { cn } from "@minanode/design-system/lib/utils";

/** Icon button that switches between the dark and light themes. Put
 * `themeInitScript` from lib/theme in <head> so a stored choice applies before
 * the first paint. */
function ThemeToggle({ className }: { className?: string }) {
  const [theme, setTheme] = React.useState<Theme>("dark");

  React.useEffect(() => {
    setTheme(
      document.documentElement.classList.contains("light") ? "light" : "dark",
    );
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
