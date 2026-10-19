import { describe, expect, it } from "vitest";
import { colorOr, isValidColor } from "./color.ts";

describe("colours (as core accepts them)", () => {
  it("accept hex and CSS names", () => {
    for (const ok of ["#f00", "#ff3b30", "#FF3B30CC", "#abcd", " red ", "RebeccaPurple", "transparent"]) {
      expect(isValidColor(ok), ok).toBe(true);
    }
  });

  it("refuse everything else", () => {
    for (const bad of ["", "#", "#ff", "#fffff", "#ggg", "rgb(1,2,3)", "red;", "url(#x)", "notacolor", "#ff3b30 x"]) {
      expect(isValidColor(bad), bad).toBe(false);
    }
  });

  it("fall back like the exporter", () => {
    expect(colorOr(" blue ", "#000")).toBe("blue");
    expect(colorOr("javascript:x", "#000")).toBe("#000");
    expect(colorOr(undefined, "#000")).toBe("#000");
  });
});
