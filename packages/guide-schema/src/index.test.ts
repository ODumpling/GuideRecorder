import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { guideSchema, parseGuide } from "./index.ts";

const repo = join(import.meta.dirname, "../../..");
const fixtures = join(repo, "fixtures/guides");
const read = (path: string) => JSON.parse(readFileSync(path, "utf8"));

describe("guideSchema", () => {
  it.each(readdirSync(fixtures))("accepts fixture %s", (bundle) => {
    const json = read(join(fixtures, bundle, "guide.json"));
    expect(parseGuide(json)).toEqual(json);
  });

  it("accepts the core test guide", () => {
    const json = read(join(repo, "crates/guiderecorder-core/testdata/guide.json"));
    expect(guideSchema.safeParse(json).success).toBe(true);
  });

  it("rejects an unknown step kind", () => {
    const json = read(join(fixtures, "Rotate an API key.guiderecorder/guide.json"));
    const step = json.items.findIndex((item: { type: string }) => item.type === "step");
    json.items[step].kind = "teleport";
    expect(guideSchema.safeParse(json).success).toBe(false);
  });

  it("rejects a guide without an id", () => {
    const json = read(join(fixtures, "Rotate an API key.guiderecorder/guide.json"));
    delete json.id;
    expect(guideSchema.safeParse(json).success).toBe(false);
  });

  it("keeps the cover and its options", () => {
    const json = read(join(fixtures, "Rotate an API key.guiderecorder/guide.json"));
    json.coverStepId = json.items.find((item: { type: string }) => item.type === "step").id;
    json.coverOptions = { image: "crop", annotations: true };
    expect(parseGuide(json)).toEqual(json);
    json.coverOptions.image = "thumbnail";
    expect(guideSchema.safeParse(json).success).toBe(false);
  });

  it("parses guide.json text", () => {
    const text = readFileSync(join(fixtures, "Rotate an API key.guiderecorder/guide.json"), "utf8");
    expect(parseGuide(text).title).toBe("Rotate an API key");
  });
});

it("SCHEMA_VERSION matches guiderecorder-core", async () => {
  const { SCHEMA_VERSION } = await import("./index.ts");
  const lib = readFileSync(join(repo, "crates/guiderecorder-core/src/lib.rs"), "utf8");
  expect(lib).toContain(`pub const SCHEMA_VERSION: u32 = ${SCHEMA_VERSION};`);
});
