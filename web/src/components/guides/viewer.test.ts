import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Guide } from "@guiderecorder/guide-schema";
import { GuideViewer } from "./GuideViewer";
import { isAllowedUrl, Markdown } from "./Markdown";
import {
  chooseImage,
  cropFits,
  drawOrder,
  hotspots,
  percentBox,
  plainText,
  slides,
  viewerItems,
  viewRect,
  type Annotation,
  type ViewerStep,
} from "./viewer";

const fixture = (name: string): Guide =>
  JSON.parse(readFileSync(new URL(`../../../../fixtures/guides/${name}.guiderecorder/guide.json`, import.meta.url), "utf8"));
const apiKey = fixture("Rotate an API key");
const floorPlan = fixture("Office floor plan");

const steps = (guide: Guide) => viewerItems(guide).filter((i): i is ViewerStep => i.type === "step");

describe("viewerItems", () => {
  it("numbers steps only, and puts each in its section", () => {
    const guide = {
      ...apiKey,
      items: [
        { type: "section", id: "s1", title: "Prepare" },
        { type: "step", id: "a", title: "One", kind: "click", image: { full: "a.webp" } },
        { type: "note", id: "n", kind: "tip", body: "Psst" },
        { type: "step", id: "b", title: "Two", kind: "click", image: { full: "b.webp" } },
      ],
    } as unknown as Guide;
    expect(viewerItems(guide)).toMatchObject([
      { type: "section", title: "Prepare" },
      { type: "step", id: "a", number: 1, section: "Prepare" },
      { type: "note", kind: "tip", body: "Psst" },
      { type: "step", id: "b", number: 2, section: "Prepare" },
    ]);
  });

  it("makes slides like the walkthrough export", () => {
    const items = viewerItems(apiKey);
    const all = slides(apiKey, items);
    expect(all.map((s) => s.kind)).toEqual(["intro", "step", "step", "step", "step", "step", "done"]);
    // The note before the first step shows on its slide.
    expect(all[1]).toMatchObject({ kind: "step", notes: [{ kind: "warning" }] });
  });
});

describe("images", () => {
  it("shows the crop where it's placed, else the full screenshot", () => {
    const rect = { x: 100, y: 50, width: 400, height: 300 };
    expect(chooseImage({ full: "f.webp", crop: "c.webp", cropRect: rect })).toEqual({ path: "c.webp", rect });
    expect(chooseImage({ full: "f.webp", crop: "c.webp", cropRect: rect }, true)).toEqual({ path: "f.webp", rect: null });
    expect(chooseImage({ full: "f.webp", crop: "c.webp" })).toEqual({ path: "f.webp", rect: null });
    expect(chooseImage({ full: "f.webp", crop: "c.webp", cropRect: rect, show: "full" })).toEqual({ path: "f.webp", rect: null });
    expect(cropFits(rect, { width: 401, height: 299 })).toBe(true);
    expect(cropFits(rect, { width: 410, height: 300 })).toBe(false);
  });

  it("cuts the view to the last crop annotation", () => {
    const image = { x: 0, y: 0, width: 1000, height: 800 };
    const crops: Annotation[] = [
      { id: "1", type: "crop", geometry: { x: 0, y: 0, width: 10, height: 10 } },
      { id: "2", type: "crop", geometry: { x: 900, y: 700, width: 400, height: 400 } },
    ];
    expect(viewRect(image, crops)).toEqual({ x: 900, y: 700, width: 100, height: 100 });
    expect(viewRect(image, [])).toBe(image);
  });

  it("draws redactions over shapes, and numbers hotspots in order", () => {
    const [first] = steps(floorPlan);
    expect(hotspots(first!.annotations).map(({ number, hotspot }) => [number, hotspot.geometry.title])).toEqual([
      [1, "Reception"],
      [2, "Kitchen & café"],
      [3, 'Meeting room "Fjord"'],
      [4, "Quiet zone"],
    ]);
    // Blur, crop, box: the blur goes over the box, the crop isn't drawn.
    const mixed = steps(apiKey)[2]!.annotations;
    expect(mixed.map((a) => a.type)).toEqual(["blur", "crop", "box"]);
    expect(drawOrder(mixed).map((a) => a.type)).toEqual(["box", "blur"]);
    expect(percentBox({ x: 250, y: 100, width: 500, height: 200 }, { x: 0, y: 0, width: 1000, height: 400 })).toEqual({
      left: "25.000%",
      top: "25.000%",
      width: "50.000%",
      height: "50.000%",
    });
  });
});

describe("Markdown", () => {
  const html = (text: string, inline = false) => renderToStaticMarkup(createElement(Markdown, { text, inline }));

  it("renders core's subset", () => {
    expect(html("Open **Settings → API keys**")).toBe('<div class="grid gap-2 [&amp;_li&gt;p]:inline "><p>Open <strong>Settings → API keys</strong></p></div>');
    expect(html("- one\n- *two*")).toContain('<ul class="list-disc pl-5"><li><p>one</p></li><li><p><em>two</em></p></li></ul>');
    expect(html("a\nb")).toContain("<p>a b</p>");
    expect(html("**Bold** title", true)).toBe("<strong>Bold</strong> title");
  });

  it("never lets raw HTML or script links through", () => {
    expect(html('<img src=x onerror="alert(1)">')).not.toContain("<img");
    expect(html("[x](javascript:alert(1))")).not.toContain("href");
    expect(html("[Handbuch](handbuch.pdf)")).toContain('href="handbuch.pdf"');
    expect(html("![alt](https://tracker.example/p.png)")).not.toContain("<img");
    expect(isAllowedUrl("mailto:a@example.com")).toBe(true);
    expect(isAllowedUrl("//evil.example")).toBe(false);
    expect(isAllowedUrl("data:text/html,x")).toBe(false);
  });

  it("gives plain text for labels", () => {
    expect(plainText("Click **Create key** and [read](x.md)")).toBe("Click Create key and read");
  });
});

describe("GuideViewer", () => {
  it("renders a guide with nothing but an image resolver", () => {
    const markup = renderToStaticMarkup(createElement(GuideViewer, { guide: floorPlan, imageUrl: (p: string) => `/img/${p}` }));
    expect(markup).toContain("Office floor plan");
    expect(markup).toContain('src="/img/steps/01M4Z96G000000000000000385.webp"');
    expect(markup).toContain("Need a desk for the day?");
  });
});
