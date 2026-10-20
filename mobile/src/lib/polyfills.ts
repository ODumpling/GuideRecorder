// Globals the shared sync code expects. The sync loop clones plain JSON
// state with structuredClone, which older Hermes builds lack.

const g = globalThis as { structuredClone?: <T>(value: T) => T };
g.structuredClone ??= <T,>(value: T): T => (value === undefined ? value : JSON.parse(JSON.stringify(value)));
