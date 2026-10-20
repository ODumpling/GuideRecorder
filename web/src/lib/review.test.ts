import { describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import {
  DEFAULT_POLICY,
  approvalProblem,
  bumpVersion,
  evaluate,
  folderOf,
  gate,
  policyFor,
  reviewStatus,
  sameContent,
  withServerFields,
  type Policy,
  type ReviewRecord,
} from "./review";

const policy = (overrides: Partial<Policy> = {}): Policy => ({ ...DEFAULT_POLICY, ...overrides });

describe("policyFor", () => {
  const policies = [policy({ folder: "", requiredApprovals: 1 }), policy({ folder: "HR", requiredApprovals: 2 }), policy({ folder: "HR/Payroll", requiredApprovals: 3 })];

  it("takes the folder's own policy, or its nearest parent's", () => {
    expect(policyFor(policies, "HR/Payroll").requiredApprovals).toBe(3);
    expect(policyFor(policies, "HR/Payroll/2026").requiredApprovals).toBe(3);
    expect(policyFor(policies, "HR/Benefits").requiredApprovals).toBe(2);
    expect(policyFor(policies, "Sales").requiredApprovals).toBe(1);
    expect(policyFor(policies, "").requiredApprovals).toBe(1);
  });

  it("doesn't match folders that only share a prefix", () => {
    expect(policyFor(policies, "HRX").requiredApprovals).toBe(1);
  });

  it("falls back to the default", () => {
    expect(policyFor([], "Anything")).toBe(DEFAULT_POLICY);
  });

  it("reads the folder off a guide row, if it has one", () => {
    expect(folderOf({})).toBe("");
    expect(folderOf({ folder: "/HR/Payroll/" })).toBe("HR/Payroll");
  });
});

describe("gate", () => {
  it("gates nothing without the approvals entitlement", () => {
    expect(gate({ enabled: false, policies: [] }, {})).toBeNull();
  });

  it("gates by the folder's policy; 0 approvals turns it off", () => {
    const approvals = { enabled: true, policies: [policy({ folder: "Drafts", requiredApprovals: 0 })] };
    expect(gate(approvals, {})).toBe(DEFAULT_POLICY);
    expect(gate(approvals, { folder: "Drafts/Old" })).toBeNull();
  });
});

describe("evaluate", () => {
  const at = (n: number) => new Date(2026, 0, 1, 0, 0, n);
  const r = (reviewerId: string, verdict: string, revisionId = "r2", n = 0): ReviewRecord => ({ reviewerId, verdict, revisionId, createdAt: at(n) });
  const request = { revisionId: "r2", authors: ["author"] };
  const anyone = () => true;

  it("needs N approvals", () => {
    const two = policy({ requiredApprovals: 2 });
    expect(evaluate(two, request, [r("a", "approve")], anyone)).toMatchObject({ approvedBy: ["a"], satisfied: false });
    expect(evaluate(two, request, [r("a", "approve"), r("b", "approve")], anyone)).toMatchObject({ satisfied: true });
  });

  it("counts each reviewer once, by their latest verdict", () => {
    const two = policy({ requiredApprovals: 2 });
    expect(evaluate(two, request, [r("a", "approve", "r2", 1), r("a", "approve", "r2", 2)], anyone).approvedBy).toEqual(["a"]);
    const changed = evaluate(two, request, [r("a", "approve", "r2", 1), r("a", "requestChanges", "r2", 2), r("b", "approve")], anyone);
    expect(changed).toMatchObject({ approvedBy: ["b"], changesRequestedBy: ["a"], satisfied: false });
    // Comments and rejections don't change a verdict.
    expect(evaluate(policy(), request, [r("a", "approve", "r2", 1), r("a", "comment", "r2", 2)], anyone).satisfied).toBe(true);
  });

  it("blocks on an outstanding request for changes", () => {
    expect(evaluate(policy(), request, [r("a", "approve"), r("b", "requestChanges")], anyone).satisfied).toBe(false);
  });

  it("only counts the M named approvers", () => {
    const named = policy({ requiredApprovals: 1, approverIds: ["a"] });
    expect(evaluate(named, request, [r("b", "approve")], anyone).satisfied).toBe(false);
    expect(evaluate(named, request, [r("a", "approve")], anyone).satisfied).toBe(true);
  });

  it("only counts reviewers whose role can still approve", () => {
    expect(evaluate(policy(), request, [r("a", "approve")], (id) => id !== "a").satisfied).toBe(false);
  });

  it("ignores the author's own approval unless self-approval is allowed", () => {
    expect(evaluate(policy(), request, [r("author", "approve")], anyone).satisfied).toBe(false);
    expect(evaluate(policy({ allowSelfApproval: true }), request, [r("author", "approve")], anyone).satisfied).toBe(true);
  });

  it("resets approvals on a new revision, or keeps them", () => {
    const old = [r("a", "approve", "r1")];
    expect(evaluate(policy({ resetOnNewRevision: true }), request, old, anyone).satisfied).toBe(false);
    expect(evaluate(policy({ resetOnNewRevision: false }), request, old, anyone).satisfied).toBe(true);
  });
});

describe("approvalProblem", () => {
  it("says why someone may not approve", () => {
    const anyone = () => true;
    expect(approvalProblem(policy(), "a", [], () => false)).toMatch(/role/);
    expect(approvalProblem(policy({ approverIds: ["b"] }), "a", [], anyone)).toMatch(/approver/);
    expect(approvalProblem(policy(), "a", ["a"], anyone)).toMatch(/own/);
    expect(approvalProblem(policy(), "a", ["b"], anyone)).toBeNull();
  });
});

describe("reviewStatus", () => {
  it("maps the server's state onto GuideStatus", () => {
    expect(reviewStatus({ headRevisionId: "r1", publishedRevisionId: null }, null)).toBe("draft");
    expect(reviewStatus({ headRevisionId: "r2", publishedRevisionId: "r1" }, null)).toBe("draft");
    expect(reviewStatus({ headRevisionId: "r2", publishedRevisionId: "r1" }, { changesRequested: false })).toBe("inReview");
    expect(reviewStatus({ headRevisionId: "r2", publishedRevisionId: "r1" }, { changesRequested: true })).toBe("needsUpdate");
    expect(reviewStatus({ headRevisionId: "r1", publishedRevisionId: "r1" }, null)).toBe("published");
  });
});

describe("bumpVersion", () => {
  it("raises the last number", () => {
    expect(bumpVersion(null)).toBe("1");
    expect(bumpVersion("")).toBe("1");
    expect(bumpVersion("1")).toBe("2");
    expect(bumpVersion("1.2")).toBe("1.3");
    expect(bumpVersion("v1.9")).toBe("v1.10");
    expect(bumpVersion("2.0 beta")).toBe("2.1 beta");
    expect(bumpVersion("beta")).toBe("beta.1");
  });
});

describe("withServerFields", () => {
  const base = { id: "g", title: "T", items: [] } as unknown as Guide;

  it("keeps the head's version and review date, whatever was pushed", () => {
    const head = { ...base, status: "published", version: "3", lastReviewed: "2026-01-01" } as Guide;
    const sent = { ...base, title: "New", status: "published", version: "9", lastReviewed: "2026-12-31" } as Guide;
    expect(withServerFields(sent, head, "draft")).toMatchObject({ title: "New", status: "draft", version: "3", lastReviewed: "2026-01-01" });
  });

  it("drops a new guide's review date", () => {
    const sent = { ...base, version: "1", lastReviewed: "2026-12-31" } as Guide;
    const result = withServerFields(sent, null, "draft");
    expect(result).toMatchObject({ status: "draft", version: "1" });
    expect("lastReviewed" in result).toBe(false);
  });

  it("is what sameContent leaves aside", () => {
    expect(sameContent({ ...base, status: "draft" }, { ...base, status: "published", version: "2" })).toBe(true);
    expect(sameContent(base, { ...base, title: "Other" })).toBe(false);
  });
});
