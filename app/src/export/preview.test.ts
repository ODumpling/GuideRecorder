import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SCORM } from "@/export/export";
import {
  frontMatterAsCode,
  markdownPreviewDocument,
  markdownPreviewHtml,
  pageLabel,
  PREVIEW_DELAY_MS,
  previewRequest,
  PreviewScheduler,
  type PreviewRequest,
  type PreviewState,
} from "@/export/preview";
import type { ExportPreviewData } from "@/lib/types";

const choices = {
  guideId: "g1",
  format: "pdf" as const,
  logo: "/Users/me/logo.png",
  accent: " #336699 ",
  paper: "letter" as const,
  reviewDetails: true,
  motion: { secondsPerStep: 5, aspect: "square" as const, zoom: false, captions: false },
};

describe("previewRequest", () => {
  it("sends the branding, paper and review details the format uses", () => {
    expect(previewRequest(choices)).toEqual({
      guideId: "g1",
      format: "pdf",
      branding: { logo: "/Users/me/logo.png", accent: "#336699", preset: null, layout: null },
      paper: "letter",
      reviewDetails: true,
      motion: { secondsPerStep: 3, aspect: "wide", zoom: true, captions: true },
      skill: { replay: false },
      scorm: DEFAULT_SCORM,
      audience: null,
    });
    // HTML has no paper; Markdown no accent.
    expect(previewRequest({ ...choices, format: "html" })?.paper).toBe("a4");
    expect(previewRequest({ ...choices, format: "md" })?.branding).toEqual({ logo: "/Users/me/logo.png", accent: null, preset: null, layout: null });
  });

  it("drops branding outside the documents and keeps the frame options of motion", () => {
    const gif = previewRequest({ ...choices, format: "gif" })!;
    expect(gif.branding).toEqual({ logo: null, accent: null, preset: null, layout: null });
    expect(gif.reviewDetails).toBe(false);
    expect(gif.motion).toEqual({ secondsPerStep: 3, aspect: "square", zoom: false, captions: false });
    expect(previewRequest({ ...choices, format: "pptx", accent: "red" })).not.toBeNull();
  });

  it("sends the audience for every format", () => {
    expect(previewRequest({ ...choices, audience: "Admin" })?.audience).toBe("Admin");
    expect(previewRequest({ ...choices, format: "mp4", audience: "Admin" })?.audience).toBe("Admin");
    expect(previewRequest({ ...choices, audience: "" })?.audience).toBeNull();
  });

  it("sends the replay choice only for skills, without branding", () => {
    const skill = previewRequest({ ...choices, format: "skill", skill: { replay: true } })!;
    expect(skill.skill).toEqual({ replay: true });
    expect(skill.branding).toEqual({ logo: null, accent: null, preset: null, layout: null });
    expect(skill.reviewDetails).toBe(false);
    expect(previewRequest({ ...choices, skill: { replay: true } })?.skill).toEqual({ replay: false });
  });

  it("sends the SCORM page's options only for SCORM, waiting for complete ones", () => {
    const scorm = { title: "Course", identifier: "c-1", completion: "quizPassed" as const, masteryScore: 60, quiz: "Q?" };
    const request = previewRequest({ ...choices, format: "scorm", scorm })!;
    // The title and identifier are only in the manifest.
    expect(request.scorm).toEqual({ ...DEFAULT_SCORM, completion: "quizPassed", masteryScore: 60, quiz: "Q?" });
    expect(request.branding).toEqual({ logo: "/Users/me/logo.png", accent: "#336699", preset: null, layout: null });
    expect(previewRequest({ ...choices, scorm })?.scorm).toEqual(DEFAULT_SCORM);
    expect(previewRequest({ ...choices, format: "scorm", scorm: { ...scorm, quiz: "" } })).toBeNull();
    expect(previewRequest({ ...choices, format: "scorm", scorm: { ...scorm, masteryScore: Number.NaN } })).toBeNull();
  });

  it("waits for a valid accent", () => {
    expect(previewRequest({ ...choices, accent: "#33669" })).toBeNull();
    expect(previewRequest({ ...choices, accent: "" })?.branding.accent).toBeNull();
  });
});

describe("PreviewScheduler", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  const pages = (n: number): ExportPreviewData => ({ kind: "pages", pages: [`p${n}`], pageCount: 1, approximate: false });
  const request = (accent: string): PreviewRequest => previewRequest({ ...choices, accent })!;

  function scheduler(make: (r: PreviewRequest) => Promise<ExportPreviewData>) {
    const states: PreviewState[] = [];
    const s = new PreviewScheduler(make, (state) => states.push(state), (e) => String(e));
    return { s, states, last: () => states.at(-1)! };
  }

  it("debounces changes into one preview of the latest options", async () => {
    const make = vi.fn(async (r: PreviewRequest) => pages(r.branding.accent!.length));
    const { s, last } = scheduler(make);
    s.request(request("#1"));
    s.request(request("#12"));
    s.request(request("#123"));
    expect(last().loading).toBe(true);
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS - 1);
    expect(make).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(make).toHaveBeenCalledTimes(1);
    expect(make.mock.calls[0]![0].branding.accent).toBe("#123");
    expect(last()).toMatchObject({ preview: pages(4), loading: false, error: null });
    // The same options again make no new preview.
    s.request(request("#123"));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(make).toHaveBeenCalledTimes(1);
  });

  it("drops answers a newer request replaced", async () => {
    let resolveFirst!: (p: ExportPreviewData) => void;
    const make = vi
      .fn<(r: PreviewRequest) => Promise<ExportPreviewData>>()
      .mockImplementationOnce(() => new Promise((resolve) => (resolveFirst = resolve)))
      .mockImplementationOnce(async () => pages(2));
    const { s, last } = scheduler(make);
    s.request(request("#111"));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    s.request(request("#222"));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(last().preview).toEqual(pages(2));
    resolveFirst(pages(1));
    await vi.advanceTimersByTimeAsync(0);
    expect(last().preview).toEqual(pages(2));
  });

  it("keeps the last preview and reports an error", async () => {
    const make = vi
      .fn<(r: PreviewRequest) => Promise<ExportPreviewData>>()
      .mockResolvedValueOnce(pages(1))
      .mockRejectedValueOnce("logo is too large");
    const { s, last } = scheduler(make);
    s.request(request("#111"));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    s.request(request("#222"));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(last()).toMatchObject({ preview: pages(1), loading: false, error: "logo is too large" });
  });

  it("ignores an invalid request and stops when disposed", async () => {
    const make = vi.fn(async () => pages(1));
    const { s, states } = scheduler(make);
    s.request(null);
    expect(states).toEqual([]);
    s.request(request("#111"));
    s.dispose();
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS * 2);
    expect(make).not.toHaveBeenCalled();
  });
});

describe("markdown preview", () => {
  it("shows a skill's front matter as code, not as a rule and a heading", () => {
    const source = '---\nname: save-a-note\ndescription: "Save a note."\n---\n\n# Save a note\n';
    expect(frontMatterAsCode(source)).toBe('```yaml\n---\nname: save-a-note\ndescription: "Save a note."\n---\n```\n\n# Save a note\n');
    expect(frontMatterAsCode("# Guide\n\n---\n")).toBe("# Guide\n\n---\n");
    const html = markdownPreviewHtml(source, []);
    expect(html).toContain('<pre><code class="language-yaml">---\nname: save-a-note');
    expect(html).not.toContain("<hr>");
    expect(html).toContain("<h1>Save a note</h1>");
  });

  it("renders the exported Markdown with the preview images", () => {
    const html = markdownPreviewHtml("# Guide\n\n![Click it](images/01-click.png)\n\n![](images/02-gone.png)\n", [
      { link: "images/01-click.png", src: "data:image/png;base64,AAAA" },
    ]);
    expect(html).toContain("<h1>Guide</h1>");
    expect(html).toContain('<img src="data:image/png;base64,AAAA" alt="Click it" />');
    expect(html).toContain('<img src="" alt="" />');
  });

  it("shows raw HTML as text and runs no script", () => {
    const html = markdownPreviewHtml("<script>alert(1)</script> and [x](javascript:alert(1))", []);
    expect(html).not.toContain("<script>");
    expect(html).not.toContain('href="javascript:');
    expect(markdownPreviewDocument(html)).toContain("default-src 'none'");
  });

  it("labels pages", () => {
    expect(pageLabel("Page", 0, 4)).toBe("Page 1 of 4");
  });
});
