import { expect, test } from "vitest";

import { buildLabel } from "./version";

test("the screens' footer: the version and the build's commit", () => {
  expect(buildLabel("0543d52a6570aaaa", "0.21.0")).toBe(
    "0.21.0, build 0543d52a6570",
  );
  expect(buildLabel(null, "0.21.0")).toBe("0.21.0");
  expect(buildLabel("0543d52a6570aaaa", null)).toBe("build 0543d52a6570");
  expect(buildLabel(null, null)).toBeNull();
});
