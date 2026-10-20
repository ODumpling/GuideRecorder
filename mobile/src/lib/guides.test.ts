import { parseGuide } from "@guiderecorder/guide-schema";
import { imagePaths } from "@guiderecorder/sync";
import { describe, expect, it } from "vitest";
import { blocks } from "./markdown";
import { appendItem, coverImage, newGuide, photoStep, plainText, stepCount, viewItems } from "./guides";
import { applyChanges } from "./revisions";

const ID = "01J00000000000000000000000";
const STEP = "01J00000000000000000000001";
const now = new Date("2026-10-20T09:00:00Z");

describe("guides made on the phone", () => {
  it("are valid guide.json", () => {
    const guide = appendItem(newGuide(ID, " Reset the router ", "en", now), photoStep(STEP, "Unplug it", "Wait **ten** seconds."), now);
    expect(() => parseGuide(guide)).not.toThrow();
    expect(guide.title).toBe("Reset the router");
    expect(guide.editedFields).toEqual(["title"]);
  });

  it("name the photo as the step's full image", () => {
    const step = photoStep(STEP, "", "");
    expect(step.image).toEqual({ full: `steps/${STEP}.jpg`, show: "full" });
    expect(step.kind).toBe("manual");
    // An empty step is left to the AI; written text is the user's.
    expect(step.aiState.status).toBe("template");
    expect(photoStep(STEP, "Title", "").aiState.status).toBe("edited");
    expect(imagePaths(appendItem(newGuide(ID, "t", "en"), step))).toEqual([`steps/${STEP}.jpg`]);
  });

  it("number steps around sections and notes", () => {
    let guide = newGuide(ID, "t", "en", now);
    guide = appendItem(guide, photoStep("01J00000000000000000000002", "a", ""));
    guide = appendItem(guide, { type: "section", id: "01J00000000000000000000003", title: "Next" });
    guide = appendItem(guide, { type: "note", id: "01J00000000000000000000004", kind: "tip", body: "x" });
    guide = appendItem(guide, photoStep("01J00000000000000000000005", "b", ""));
    expect(viewItems(guide).flatMap((i) => (i.type === "step" ? [i.number] : []))).toEqual([1, 2]);
    expect(stepCount(guide)).toBe(2);
    expect(coverImage(guide)).toBe("steps/01J00000000000000000000002.jpg");
  });

  it("use the cover the author chose, else the first step", () => {
    let guide = newGuide(ID, "t", "en", now);
    guide = appendItem(guide, photoStep("01J00000000000000000000002", "a", ""));
    guide = appendItem(guide, { ...photoStep("01J00000000000000000000003", "b", ""), image: { full: "steps/b.jpg", crop: "steps/b.crop.jpg", show: "crop" } });
    expect(coverImage({ ...guide, coverStepId: "01J00000000000000000000003" })).toBe("steps/b.jpg");
    expect(coverImage({ ...guide, coverStepId: "01J00000000000000000000003", coverOptions: { image: "crop", annotations: false } })).toBe("steps/b.crop.jpg");
    expect(coverImage({ ...guide, coverStepId: "01J0000000000000000000GONE" })).toBe("steps/01J00000000000000000000002.jpg");
  });
});

describe("applyChanges", () => {
  it("keeps the latest revision and drops deleted guides", () => {
    const revision = { id: "r2", guideId: ID, parentId: "r1", guide: newGuide(ID, "t", "en"), assets: {}, authorId: null, source: "app", createdAt: "" } as const;
    const after = applyChanges({ other: { revision: { ...revision, guideId: "other" } } }, [
      { type: "revision", guideId: ID, seq: "2", revision },
      { type: "deleted", guideId: "other", seq: "3", deletedAt: "", purgeAt: "" },
    ]);
    expect(Object.keys(after)).toEqual([ID]);
    expect(after[ID]!.revision.id).toBe("r2");
  });
});

describe("markdown", () => {
  it("splits paragraphs and lists", () => {
    expect(blocks("One\ntwo\n\n- a\n- b\n1. c")).toEqual([
      { type: "p", text: "One two" },
      { type: "list", ordered: false, items: ["a", "b"] },
      { type: "list", ordered: true, items: ["c"] },
    ]);
    expect(plainText("Click **Save** [here](https://x)")).toBe("Click Save here");
  });
});
