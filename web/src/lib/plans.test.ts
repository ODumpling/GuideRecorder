import { describe, expect, it } from "vitest";
import { planOf } from "./plans";

describe("planOf", () => {
  it("is free without subscriptions", () => {
    expect(planOf([])).toBe("free");
  });

  it("ignores subscriptions that aren't active", () => {
    expect(planOf([{ plan: "pro", status: "canceled" }, { plan: "enterprise", status: "incomplete" }])).toBe("free");
  });

  it("picks the highest active plan", () => {
    expect(planOf([{ plan: "pro", status: "active" }, { plan: "enterprise", status: "trialing" }])).toBe("enterprise");
    expect(planOf([{ plan: "pro", status: "past_due" }])).toBe("pro");
  });
});
