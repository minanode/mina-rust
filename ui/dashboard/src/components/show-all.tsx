import * as React from "react";

import { Button } from "@minanode/design-system/components/button";
import { formatCount } from "@minanode/node-client";

/** Rows a long list shows before "Show all", on every page. */
export const ROWS_SHOWN = 10;

/**
 * The first `ROWS_SHOWN` of a long list, and a button for a section's aside
 * that shows all of them or the first again. `first` names the end the list
 * starts from: "newest", "soonest".
 */
export function useShowAll<T>(items: readonly T[], first: string) {
  const [all, setAll] = React.useState(false);
  const long = items.length > ROWS_SHOWN;
  return {
    shown: all || !long ? items : items.slice(0, ROWS_SHOWN),
    hidden: all || !long ? 0 : items.length - ROWS_SHOWN,
    toggle: long ? (
      <Button
        type="button"
        variant="outline"
        size="sm"
        aria-expanded={all}
        onClick={() => setAll(a => !a)}
      >
        {all
          ? `Show the ${first} ${ROWS_SHOWN}`
          : `Show all ${formatCount(items.length)}`}
      </Button>
    ) : null,
  };
}
