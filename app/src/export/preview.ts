// The Export dialog's preview: what to ask the exporter for, when, and how
// to show a Markdown preview. The preview itself is made on the Rust side
// by each format's own exporter (`preview_export`), so it shows the real
// logo, page breaks, layout and burned-in blurs.

import MarkdownIt from "markdown-it";
import { isAccent, isDocumentFormat, isMotionFormat } from "@/export/export";
import type { ExportBranding, ExportFormat, ExportPreviewData, MotionOptions, PaperSize } from "@/lib/types";

/** How long the options must stay unchanged before a new preview is made, in ms. */
export const PREVIEW_DELAY_MS = 300;

/** What a preview is made from: the export's options, without a destination. */
export interface PreviewRequest {
  guideId: string;
  format: ExportFormat;
  branding: ExportBranding;
  paper: PaperSize;
  reviewDetails: boolean;
  motion: MotionOptions;
}

/**
 * The preview request for the dialog's choices, with what the format
 * ignores left out (so changing it makes no new preview), as the export
 * sends them. Null while the accent is not a valid colour.
 */
export function previewRequest(choices: {
  guideId: string;
  format: ExportFormat;
  logo: string | null;
  accent: string;
  paper: PaperSize;
  reviewDetails: boolean;
  motion: MotionOptions;
}): PreviewRequest | null {
  const { guideId, format } = choices;
  const document = isDocumentFormat(format);
  if (document && !isAccent(choices.accent)) return null;
  return {
    guideId,
    format,
    branding: document
      ? { logo: choices.logo, accent: format === "md" ? null : choices.accent.trim() || null }
      : { logo: null, accent: null },
    paper: format === "pdf" || format === "docx" ? choices.paper : "a4",
    reviewDetails: document && choices.reviewDetails,
    // Seconds per step change no still; only the frame and captions do.
    motion: isMotionFormat(format)
      ? { ...choices.motion, secondsPerStep: 3 }
      : { secondsPerStep: 3, aspect: "wide", zoom: true, captions: true },
  };
}

/** A preview as the dialog shows it. */
export interface PreviewState {
  preview: ExportPreviewData | null;
  /** The request `preview` was made for. */
  key: string | null;
  loading: boolean;
  error: string | null;
}

/**
 * Makes previews as the options change: waits until they have stayed the
 * same for `delay` ms (so typing an accent or flicking through options
 * makes one preview, not many), skips a request equal to the last one,
 * and drops the answer to any request a newer one replaced.
 */
export class PreviewScheduler {
  private timer: ReturnType<typeof setTimeout> | null = null;
  private latest = 0;
  private key: string | null = null;
  private state: PreviewState = { preview: null, key: null, loading: false, error: null };

  constructor(
    private readonly make: (request: PreviewRequest) => Promise<ExportPreviewData>,
    private readonly onState: (state: PreviewState) => void,
    private readonly describeError: (error: unknown) => string,
    private readonly delay = PREVIEW_DELAY_MS,
  ) {}

  /** Asks for a preview of `request` (null: keep the last one). */
  request(request: PreviewRequest | null) {
    if (!request) return;
    const key = JSON.stringify(request);
    if (key === this.key) return;
    this.key = key;
    this.cancelTimer();
    const id = ++this.latest;
    this.set({ loading: true });
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.make(request).then(
        (preview) => id === this.latest && this.set({ preview, key, loading: false, error: null }),
        (error) => id === this.latest && this.set({ loading: false, error: this.describeError(error) }),
      );
    }, this.delay);
  }

  /** Stops: no pending preview is made and no answer is reported. */
  dispose() {
    this.cancelTimer();
    this.latest++;
  }

  private cancelTimer() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  private set(change: Partial<PreviewState>) {
    this.state = { ...this.state, ...change };
    this.onState(this.state);
  }
}

/** "Page 2 of 5". */
export function pageLabel(noun: string, index: number, count: number): string {
  return `${noun} ${index + 1} of ${count}`;
}

// CommonMark, as the exported Markdown is; raw HTML in it shows as text.
const markdown = new MarkdownIt("commonmark", { html: false });

/**
 * The exported Markdown rendered as HTML, with each image link (`images/…`)
 * replaced by its preview image; an image the preview lacks is left out.
 */
export function markdownPreviewHtml(source: string, images: { link: string; src: string }[]): string {
  const sources = new Map(images.map((image) => [image.link, image.src]));
  const tokens = markdown.parse(source, {});
  for (const token of tokens) {
    for (const child of token.children ?? []) {
      if (child.type === "image") child.attrSet("src", sources.get(String(child.attrGet("src") ?? "")) ?? "");
    }
  }
  return markdown.renderer.render(tokens, markdown.options, {});
}

/** The page a Markdown preview shows in its frame: `body` with a GitHub-like look and no script. */
export function markdownPreviewDocument(body: string): string {
  return `<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'">
<style>
body{margin:0;padding:28px 32px;font:15px/1.6 -apple-system,BlinkMacSystemFont,"Helvetica Neue",sans-serif;color:#1f2328;background:#fff}
h1,h2,h3{line-height:1.25;margin:1.4em 0 .6em}h1{font-size:2em;border-bottom:1px solid #d1d9e0;padding-bottom:.3em}
h2{font-size:1.5em;border-bottom:1px solid #d1d9e0;padding-bottom:.3em}h3{font-size:1.25em}
img{max-width:100%;border:1px solid #d1d9e0;border-radius:4px}a{color:#0969da}
code{background:#eff1f3;border-radius:4px;padding:.15em .35em;font-size:85%}
blockquote{margin:0;padding:0 1em;color:#59636e;border-left:.25em solid #d1d9e0}
</style></head><body>${body}</body></html>`;
}
