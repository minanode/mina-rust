import * as React from "react";

/** Whether a scroll box has more content past its right edge, kept up to
 * date as it scrolls and resizes: a table or a tab strip wider than its box
 * fades there while it does, so a sideways scroll is discoverable. */
export function useMoreToTheRight<T extends HTMLElement>() {
  const ref = React.useRef<T>(null);
  const [right, setRight] = React.useState(false);
  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const check = () =>
      setRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 1);
    check();
    el.addEventListener("scroll", check, { passive: true });
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(check);
    observer?.observe(el);
    if (el.firstElementChild) observer?.observe(el.firstElementChild);
    return () => {
      el.removeEventListener("scroll", check);
      observer?.disconnect();
    };
  }, []);
  return { ref, right };
}
