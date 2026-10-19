/**
 * `tags` with the tags typed in `text` added at the end: split on commas
 * and line breaks, trimmed, with spaces collapsed. Empty ones and ones
 * already there (ignoring case) are dropped.
 */
export function addTags(tags: string[], text: string): string[] {
  const seen = new Set(tags.map((tag) => tag.toLowerCase()));
  const added: string[] = [];
  for (const part of text.split(/[,\n]/)) {
    const tag = part.trim().replace(/\s+/g, " ");
    if (!tag || seen.has(tag.toLowerCase())) continue;
    seen.add(tag.toLowerCase());
    added.push(tag);
  }
  return added.length ? [...tags, ...added] : tags;
}

/** `tags` without the one at `index`. */
export function removeTag(tags: string[], index: number): string[] {
  return tags.filter((_, i) => i !== index);
}
