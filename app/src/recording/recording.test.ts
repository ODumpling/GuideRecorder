import { describe, expect, it } from "vitest";
import { savedNotice } from "@/recording/recording";

const none = { skipped: 0, suggestedBlurs: 0, appliedBlurs: 0, unscannedSteps: 0 };

describe("savedNotice", () => {
  it("says nothing when there is nothing to say", () => {
    expect(savedNotice(none)).toBeNull();
  });

  it("tells the user to check steps that could not be scanned", () => {
    expect(savedNotice({ ...none, unscannedSteps: 1 })).toBe(
      "1 step couldn't be scanned for sensitive text; check it before sharing.",
    );
    expect(savedNotice({ ...none, suggestedBlurs: 2, unscannedSteps: 3 })).toBe(
      "2 areas look sensitive: review the suggested blurs. 3 steps couldn't be scanned for sensitive text; check them before sharing.",
    );
  });
});
