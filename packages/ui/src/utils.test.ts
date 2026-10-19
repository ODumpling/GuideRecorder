import { describe, expect, it } from "vitest";
import { cn } from "./utils.ts";

describe("cn", () => {
  it("joins class names and skips falsy values", () => {
    expect(cn("px-2", false, undefined, "text-sm", { hidden: false, flex: true })).toBe(
      "px-2 text-sm flex",
    );
  });

  it("lets later Tailwind classes override conflicting earlier ones", () => {
    expect(cn("px-2 py-1", "px-4")).toBe("py-1 px-4");
  });
});
