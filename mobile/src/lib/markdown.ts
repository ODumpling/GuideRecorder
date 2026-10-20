// The block structure of guide Markdown: paragraphs and lists.

export type Block = { type: "p"; text: string } | { type: "list"; ordered: boolean; items: string[] };

export function blocks(markdown: string): Block[] {
  const out: Block[] = [];
  for (const chunk of markdown.trim().split(/\n\s*\n/)) {
    const lines = chunk.split("\n");
    let list: Extract<Block, { type: "list" }> | null = null;
    let para: string[] = [];
    const flush = () => {
      if (para.length) out.push({ type: "p", text: para.join(" ") });
      para = [];
    };
    for (const line of lines) {
      const item = /^\s*(?:([-*+])|(\d+)[.)])\s+(.*)$/.exec(line);
      if (item) {
        flush();
        const ordered = Boolean(item[2]);
        if (!list || list.ordered !== ordered) {
          list = { type: "list", ordered, items: [] };
          out.push(list);
        }
        list.items.push(item[3]!);
      } else if (list && /^\s+\S/.test(line)) {
        list.items[list.items.length - 1] += ` ${line.trim()}`;
      } else {
        list = null;
        if (line.trim()) para.push(line.trim());
      }
    }
    flush();
  }
  return out;
}
