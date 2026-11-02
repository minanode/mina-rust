/** The banner of a tab that couldn't open the frontier store, because
 * another tab of this browser holds it: this tab runs a second node. */

import { Banner } from "@minanode/design-system/patterns/banner";
import { useNodeStatus } from "@minanode/node-client/react";

export function SecondTabBanner() {
  const { status } = useNodeStatus();
  if (!status.data?.frontierStore?.locked) return null;
  return (
    <Banner tone="caution" title="Another tab runs this browser's node.">
      This tab runs a second one without the saved chain, so the browser uses
      about twice the memory.
    </Banner>
  );
}
