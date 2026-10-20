import MarkdownIt, { type Token } from "markdown-it";
import { useMemo, type ReactNode } from "react";

// Guide text (core's Markdown subset: paragraphs, lists, emphasis, code,
// links and line breaks; see crates/guiderecorder-core/src/markdown.rs)
// rendered to React elements, never to HTML strings. Other constructs
// degrade as core's exporters degrade them: headings and code blocks become
// paragraphs, images their alt text, raw HTML literal text.


const parser = new MarkdownIt({ html: false, linkify: false });

/** Core's rule for links: http, https, mailto or relative, no whitespace or protocol-relative prefix. */
export function isAllowedUrl(url: string): boolean {
  if (!url || /[\s\u0000-\u001f\u007f]/.test(url) || /^[/\\][/\\]/.test(url)) return false;
  const i = url.search(/[:/?#]/);
  if (i >= 0 && url[i] === ":") return ["http", "https", "mailto"].includes(url.slice(0, i).toLowerCase());
  return true;
}

function inlines(tokens: Token[]): ReactNode[] {
  const out: ReactNode[] = [];
  const stack: { tag: string; href?: string; children: ReactNode[] }[] = [];
  const push = (node: ReactNode) => (stack.at(-1)?.children ?? out).push(node);
  for (const [i, t] of tokens.entries()) {
    switch (t.type) {
      case "text":
      case "html_inline":
        push(t.content);
        break;
      case "code_inline":
        push(
          <code key={i} className="rounded bg-muted px-1 py-0.5 text-[0.9em]">
            {t.content}
          </code>,
        );
        break;
      case "softbreak":
        push(" ");
        break;
      case "hardbreak":
        push(<br key={i} />);
        break;
      case "image":
        push(t.content);
        break;
      case "strong_open":
      case "em_open":
      case "s_open":
      case "link_open":
        stack.push({ tag: t.type, href: String(t.attrGet("href") ?? "") || undefined, children: [] });
        break;
      case "strong_close":
      case "em_close":
      case "s_close":
      case "link_close": {
        const open = stack.pop();
        if (!open) break;
        const children = open.children;
        if (open.tag === "strong_open") push(<strong key={i}>{children}</strong>);
        else if (open.tag === "em_open") push(<em key={i}>{children}</em>);
        else if (open.tag === "s_open") push(<s key={i}>{children}</s>);
        else if (open.href && isAllowedUrl(open.href))
          push(
            <a key={i} href={open.href} target="_blank" rel="noopener noreferrer nofollow" className="text-brand underline underline-offset-2">
              {children}
            </a>,
          );
        else push(<span key={i}>{children}</span>);
        break;
      }
    }
  }
  // Unclosed wrappers (shouldn't happen): keep their text.
  while (stack.length) out.push(...stack.shift()!.children);
  return out;
}

function blocks(tokens: Token[]): ReactNode[] {
  const out: ReactNode[] = [];
  const stack: { tag: "ul" | "ol" | "li"; start?: number; children: ReactNode[] }[] = [];
  const push = (node: ReactNode) => (stack.at(-1)?.children ?? out).push(node);
  for (const [i, t] of tokens.entries()) {
    switch (t.type) {
      case "inline":
        push(<p key={i}>{inlines(t.children ?? [])}</p>);
        break;
      case "code_block":
      case "fence":
        push(
          <p key={i} className="whitespace-pre-wrap font-mono text-[0.9em]">
            {t.content.replace(/\n$/, "")}
          </p>,
        );
        break;
      case "html_block":
        push(<p key={i}>{t.content}</p>);
        break;
      case "bullet_list_open":
        stack.push({ tag: "ul", children: [] });
        break;
      case "ordered_list_open":
        stack.push({ tag: "ol", start: Number(t.attrGet("start") ?? 1), children: [] });
        break;
      case "list_item_open":
        stack.push({ tag: "li", children: [] });
        break;
      case "bullet_list_close":
      case "ordered_list_close":
      case "list_item_close": {
        const open = stack.pop();
        if (!open) break;
        if (open.tag === "li") push(<li key={i}>{open.children}</li>);
        else if (open.tag === "ul") push(<ul key={i} className="list-disc pl-5">{open.children}</ul>);
        else
          push(
            <ol key={i} start={open.start} className="list-decimal pl-5">
              {open.children}
            </ol>,
          );
        break;
      }
    }
  }
  return out;
}

/** Guide Markdown as blocks; `inline` renders the first paragraph's content only (titles). */
export function Markdown({ text, inline = false, className }: { text: string; inline?: boolean; className?: string }) {
  const nodes = useMemo(() => {
    if (inline) return inlines(parser.parseInline(text, {})[0]?.children ?? []);
    return blocks(parser.parse(text, {}));
  }, [text, inline]);
  if (inline) return <>{nodes}</>;
  return <div className={`grid gap-2 [&_li>p]:inline ${className ?? ""}`}>{nodes}</div>;
}
