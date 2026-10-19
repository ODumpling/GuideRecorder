// The guide.json format (guiderecorder_core::model) for the web stack:
// TypeScript types and a Zod schema, both generated from the Rust model.

import { z } from "zod";
import { guideJsonSchema, type Guide } from "./guide.gen.ts";

export * from "./guide.gen.ts";

/** The guide.json schema version this package was generated for. */
export const SCHEMA_VERSION = 2;

/** Validates a parsed `guide.json`. */
export const guideSchema = z.fromJSONSchema(
  guideJsonSchema as unknown as Parameters<typeof z.fromJSONSchema>[0],
) as unknown as z.ZodType<Guide>;

/** Parses `guide.json` text or a parsed value, throwing a ZodError if invalid. */
export function parseGuide(input: string | unknown): Guide {
  return guideSchema.parse(typeof input === "string" ? JSON.parse(input) : input);
}
