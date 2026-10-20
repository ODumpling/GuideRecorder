CREATE TABLE "export_job" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"guide_id" text NOT NULL,
	"revision_id" text NOT NULL,
	"format" text NOT NULL,
	"status" text DEFAULT 'pending' NOT NULL,
	"key" text,
	"error" text,
	"attempts" integer DEFAULT 0 NOT NULL,
	"requested_by" text,
	"created_at" timestamp DEFAULT now() NOT NULL,
	"started_at" timestamp,
	"finished_at" timestamp
);
--> statement-breakpoint
ALTER TABLE "guide" ADD COLUMN "folder" text;--> statement-breakpoint
ALTER TABLE "guide" ADD COLUMN "search" "tsvector";--> statement-breakpoint
ALTER TABLE "export_job" ADD CONSTRAINT "export_job_revision_id_revision_id_fk" FOREIGN KEY ("revision_id") REFERENCES "public"."revision"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "export_job" ADD CONSTRAINT "export_job_requested_by_user_id_fk" FOREIGN KEY ("requested_by") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
CREATE UNIQUE INDEX "export_job_revision_format_idx" ON "export_job" USING btree ("revision_id","format");--> statement-breakpoint
CREATE INDEX "export_job_status_idx" ON "export_job" USING btree ("status","created_at");--> statement-breakpoint
CREATE INDEX "guide_search_idx" ON "guide" USING gin ("search");--> statement-breakpoint
-- Index guides synced before search existed, the way lib/search.ts does on push.
UPDATE "guide" SET "search" =
	setweight(to_tsvector('simple', concat_ws(' ', r.guide_json->>'title',
		(SELECT string_agg(t, ' ') FROM jsonb_array_elements_text(coalesce(r.guide_json->'tags', '[]')) t))), 'A') ||
	setweight(to_tsvector('simple', concat_ws(' ', r.guide_json->>'summary',
		(SELECT string_agg(i->>'title', ' ') FROM jsonb_array_elements(coalesce(r.guide_json->'items', '[]')) i WHERE i->>'type' IN ('step', 'section')))), 'B') ||
	setweight(to_tsvector('simple', concat_ws(' ', r.guide_json->>'intro', r.guide_json->>'prerequisites',
		(SELECT string_agg(concat_ws(' ', i->>'description', i->>'body'), ' ') FROM jsonb_array_elements(coalesce(r.guide_json->'items', '[]')) i),
		(SELECT string_agg(concat_ws(' ', a->'geometry'->>'title', a->'geometry'->>'description', a->'geometry'->>'text'), ' ')
			FROM jsonb_array_elements(coalesce(r.guide_json->'items', '[]')) i, jsonb_array_elements(coalesce(i->'annotations', '[]')) a))), 'C')
FROM "revision" r
WHERE r."id" = "guide"."head_revision_id";