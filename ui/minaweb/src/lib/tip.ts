/**
 * Keeps the header's best tip level with the page under it. The header
 * reads the shared `/status`, polled once for the whole shell; a page
 * polls its own data from the moment it mounts. While the tab is hidden
 * no poll asks again, and a page opened then (or one whose poll answered
 * first) can show a block the header's last `/status` predates. So the
 * shell asks `/status` again on every navigation, and a page that holds a
 * newer block than the header asks it again too.
 */

import * as React from "react";

import { useNodeStatus } from "@minanode/node-client/react";

import { usePathname } from "../app/router";

/** The shared `/status` is behind a block the page holds. */
export function statusBehind(
  statusHeight: number | null,
  seenHeight: number | null,
): boolean {
  return (
    statusHeight !== null && seenHeight !== null && seenHeight > statusHeight
  );
}

/** Asks `/status` again when the page holds a block above its best tip,
 * once for each pair of heights. */
export function useStatusCatchUp(seenHeight: number | null) {
  const { status } = useNodeStatus();
  const statusHeight = status.data?.bestTip?.height ?? null;
  const behind = statusBehind(statusHeight, seenHeight);
  const { refresh } = status;
  React.useEffect(() => {
    if (behind) refresh();
  }, [behind, statusHeight, seenHeight, refresh]);
}

/** Asks `/status` again on every navigation after the first page, so the
 * header and the new page answer from the same moment. */
export function useStatusOnNavigate() {
  const pathname = usePathname();
  const { status } = useNodeStatus();
  const { refresh } = status;
  const shown = React.useRef(pathname);
  React.useEffect(() => {
    if (shown.current === pathname) return;
    shown.current = pathname;
    refresh();
  }, [pathname, refresh]);
}
