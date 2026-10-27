import * as React from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { Alert } from "../components/alert";
import { flatInSurface, borderlessInSurface } from "../lib/surface";
import { AppShell, SidebarNav, SidebarNavItem } from "./app-shell";
import { CopyButton } from "./copy-button";
import { DataAge } from "./data-age";
import { EmptyState } from "./empty-state";
import { InlineConfirm } from "./inline-confirm";
import { RelativeTime } from "./relative-time";
import { SegmentedControl } from "./segmented-control";
import { StageBar } from "./stage-bar";
import { Stat } from "./stat";
import { StatRow } from "./stat-row";
import { TextLink } from "./text-link";
import { Timeline } from "./timeline";

afterEach(cleanup);

/** An app's own link component, as a router would give it. */
const RouterLink = React.forwardRef<
  HTMLAnchorElement,
  React.ComponentProps<"a"> & { to: string }
>(({ to, ...props }, ref) => (
  <a ref={ref} href={`#${to}`} data-router {...props} />
));

test("TextLink asChild styles the app's link and puts the arrow inside it", () => {
  render(
    <TextLink asChild arrow className="text-sm">
      <RouterLink to="/mempool">Mempool</RouterLink>
    </TextLink>,
  );
  const link = screen.getByRole("link", { name: "Mempool" });
  expect(link.hasAttribute("data-router")).toBe(true);
  expect(link.className).toContain("text-brand-text");
  expect(link.className).toContain("text-sm");
  expect(link.querySelector("svg")).not.toBeNull();
});

test("InlineConfirm: confirm, cancel, and busy", () => {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  const { rerender } = render(
    <InlineConfirm
      label="Confirm stopping"
      message="Stop it?"
      confirmLabel="Stop"
      busyLabel="Stopping…"
      cancelLabel="Keep it"
      onConfirm={onConfirm}
      onCancel={onCancel}
    />,
  );
  expect(screen.getByRole("group", { name: "Confirm stopping" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Stop" }));
  fireEvent.click(screen.getByRole("button", { name: "Keep it" }));
  expect(onConfirm).toHaveBeenCalledOnce();
  expect(onCancel).toHaveBeenCalledOnce();
  rerender(
    <InlineConfirm
      label="Confirm stopping"
      message="Stop it?"
      confirmLabel="Stop"
      busyLabel="Stopping…"
      cancelLabel="Keep it"
      busy
      onConfirm={onConfirm}
      onCancel={onCancel}
    />,
  );
  const busy = screen.getByRole("button", {
    name: "Stopping…",
  }) as HTMLButtonElement;
  expect(busy.disabled).toBe(true);
  expect(
    (screen.getByRole("button", { name: "Keep it" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
});

test("CopyButton names what it copies, and each state", () => {
  render(
    <>
      <CopyButton text="x" label="public key" />
      <CopyButton text="x" label="state hash" state="copied" />
      <CopyButton text="x" label="peer ID" state="failed" />
      <CopyButton variant="text" text="{}" label="Node status">
        Copy JSON
      </CopyButton>
    </>,
  );
  expect(screen.getByRole("button", { name: "Copy public key" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Copied" })).toBeTruthy();
  expect(
    screen.getByRole("button", {
      name: "Couldn't copy: select the text instead",
    }),
  ).toBeTruthy();
  expect(
    screen.getByRole("button", { name: /Copy JSON\s*of Node status/ }),
  ).toBeTruthy();
});

test("SegmentedControl marks the choice and reports a new one", () => {
  const onValueChange = vi.fn();
  render(
    <SegmentedControl
      label="Which peers"
      options={[
        { value: "connected", label: "Connected" },
        { value: "all", label: "All" },
      ]}
      value="connected"
      onValueChange={onValueChange}
    />,
  );
  expect(screen.getByRole("group", { name: "Which peers" })).toBeTruthy();
  expect(
    screen
      .getByRole("button", { name: "Connected" })
      .getAttribute("aria-pressed"),
  ).toBe("true");
  expect(
    screen.getByRole("button", { name: "All" }).getAttribute("aria-pressed"),
  ).toBe("false");
  fireEvent.click(screen.getByRole("button", { name: "All" }));
  expect(onValueChange).toHaveBeenCalledWith("all");
});

test("DataAge: loading, fresh, stale, paused", () => {
  const now = Date.now();
  const { rerender } = render(
    <DataAge updatedAt={null} loading staleAfterMs={20_000} />,
  );
  expect(screen.getByText("Loading…")).toBeTruthy();
  rerender(
    <DataAge
      updatedAt={now - 3_000}
      staleAfterMs={20_000}
      onRefresh={() => {}}
    />,
  );
  expect(screen.getByText(/^Updated \d+ s ago$/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Refresh" })).toBeTruthy();
  rerender(<DataAge updatedAt={now - 125_000} staleAfterMs={20_000} />);
  expect(screen.getByText(/^Data from 2 min \d+ s ago$/)).toBeTruthy();
  rerender(<DataAge updatedAt={now - 125_000} paused staleAfterMs={20_000} />);
  expect(screen.getByText(/^Paused, data from 2 min \d+ s ago$/)).toBeTruthy();
});

test("Stat labelled inList renders a term and its descriptions", () => {
  const { container } = render(
    <dl>
      <Stat
        variant="labelled"
        inList
        label="Peers"
        value="14"
        detail="connected"
      />
    </dl>,
  );
  expect(container.querySelector("dt")?.textContent).toBe("Peers");
  expect([...container.querySelectorAll("dd")].map(d => d.textContent)).toEqual(
    ["14", "connected"],
  );
});

test("StatRow lists its figures as terms and descriptions", () => {
  const { container } = render(
    <StatRow
      label="Outcomes"
      items={[
        { label: "Won", value: "7" },
        { label: "Canonical", value: "3", detail: "of 4 sent" },
      ]}
    />,
  );
  expect(screen.getByRole("group", { name: "Outcomes" })).toBeTruthy();
  expect([...container.querySelectorAll("dt")].map(d => d.textContent)).toEqual(
    ["Won", "Canonical"],
  );
  expect([...container.querySelectorAll("dd")].map(d => d.textContent)).toEqual(
    ["7", "3", "of 4 sent"],
  );
});

test("StageBar is one named image, its parts placed on the domain", () => {
  const { container } = render(
    <StageBar
      label="Proof from 6 s before the slot to 3 s into it"
      domain={[-10, 90]}
      window={{ start: 0, end: 90 }}
      spans={[{ start: -6, end: 3 }]}
      marks={[{ at: 40 }]}
      ticks={[
        { at: 0, label: "0 s" },
        { at: 90, label: "90 s" },
      ]}
    />,
  );
  const bar = screen.getByRole("img", {
    name: "Proof from 6 s before the slot to 3 s into it",
  });
  const part = (name: string) =>
    bar.querySelector<HTMLElement>(`[data-part=${name}]`)!.style;
  expect(part("window").left).toBe("10%");
  expect(part("window").width).toBe("90%");
  expect(part("span").left).toBe("4%");
  expect(part("span").width).toBe("9%");
  expect(part("mark").left).toBe("50%");
  expect(container.textContent).toBe("0 s90 s");
});

test("Timeline says in words how each step went", () => {
  render(
    <Timeline
      label="Slot 100"
      steps={[
        { label: "Built", time: "−5.8 s", state: "done" },
        { label: "Proof done", state: "stopped" },
        { label: "Broadcast", state: "skipped" },
      ]}
    />,
  );
  const items = screen
    .getByRole("list", { name: "Slot 100" })
    .querySelectorAll("li");
  expect([...items].map(li => li.textContent)).toEqual([
    "Built−5.8 s",
    "Proof donestopped here",
    "Broadcastnot reached",
  ]);
});

test("AppShell's menu opens, marks the current page, and closes on Escape", () => {
  render(
    <AppShell
      brand={<span>Brand</span>}
      navLabel="App"
      nav={
        <SidebarNav>
          <SidebarNavItem href="#/" current>
            Home
          </SidebarNavItem>
          <SidebarNavItem href="#/other">Other</SidebarNavItem>
        </SidebarNav>
      }
    >
      <p>Page</p>
    </AppShell>,
  );
  // The sidebar's copy of the navigation is always there; the menu adds one.
  expect(screen.getAllByRole("navigation", { name: "App" })).toHaveLength(1);
  const open = screen.getByRole("button", { name: "Open menu" });
  fireEvent.click(open);
  expect(open.getAttribute("aria-expanded")).toBe("true");
  expect(screen.getAllByRole("navigation", { name: "App" })).toHaveLength(2);
  expect(
    screen
      .getAllByRole("link", { name: "Home" })
      .every(a => a.getAttribute("aria-current") === "page"),
  ).toBe(true);
  fireEvent.keyDown(document, { key: "Escape" });
  expect(
    screen
      .getByRole("button", { name: "Open menu" })
      .getAttribute("aria-expanded"),
  ).toBe("false");
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: "Open menu" }),
  );
});

test("SidebarNavItem closes the menu when followed", () => {
  render(
    <AppShell
      brand={<span>Brand</span>}
      navLabel="App"
      nav={
        <SidebarNav>
          <SidebarNavItem href="#/other">Other</SidebarNavItem>
        </SidebarNav>
      }
    >
      <p>Page</p>
    </AppShell>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Open menu" }));
  const links = screen.getAllByRole("link", { name: "Other" });
  fireEvent.click(links[links.length - 1]);
  expect(
    screen
      .getByRole("button", { name: "Open menu" })
      .getAttribute("aria-expanded"),
  ).toBe("false");
});

test("RelativeTime says ago for the past and in for the future", () => {
  const now = Date.now();
  render(
    <>
      <RelativeTime at={now - 125_000} />
      <RelativeTime at={now + 25 * 60_000 + 30_000} />
      <RelativeTime at={null} />
    </>,
  );
  expect(screen.getByText(/^2 min \d+ s ago$/)).toBeTruthy();
  expect(screen.getByText(/^in 25 min \d+ s$/)).toBeTruthy();
  expect(screen.getByText("–")).toBeTruthy();
});

test("box patterns lose their border inside a surface", () => {
  const { container } = render(
    <>
      <EmptyState title="Empty" />
      <Alert>Note</Alert>
      <InlineConfirm
        label="Confirm"
        message="Sure?"
        confirmLabel="Yes"
        onConfirm={() => {}}
        onCancel={() => {}}
      />
      <StatRow items={[{ label: "Won", value: "7" }]} />
    </>,
  );
  const has = (slot: string, classes: string) =>
    classes
      .split(" ")
      .every(c =>
        container.querySelector(`[data-slot=${slot}]`)!.classList.contains(c),
      );
  expect(has("empty-state", flatInSurface)).toBe(true);
  expect(has("alert", borderlessInSurface)).toBe(true);
  expect(has("stat-row", borderlessInSurface)).toBe(true);
  expect(
    screen
      .getByRole("group", { name: "Confirm" })
      .className.includes("in-data-[slot=card]:border-0"),
  ).toBe(true);
});
