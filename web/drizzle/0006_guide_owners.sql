CREATE TABLE "guide_owner" (
	"organization_id" text NOT NULL,
	"guide_id" text NOT NULL,
	"user_id" text NOT NULL,
	"added_by" text,
	"created_at" timestamp DEFAULT now() NOT NULL,
	CONSTRAINT "guide_owner_organization_id_guide_id_user_id_pk" PRIMARY KEY("organization_id","guide_id","user_id")
);
--> statement-breakpoint
ALTER TABLE "guide" ADD COLUMN "locked" boolean DEFAULT false NOT NULL;--> statement-breakpoint
ALTER TABLE "guide_owner" ADD CONSTRAINT "guide_owner_user_id_user_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."user"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "guide_owner" ADD CONSTRAINT "guide_owner_added_by_user_id_fk" FOREIGN KEY ("added_by") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "guide_owner" ADD CONSTRAINT "guide_owner_organization_id_guide_id_guide_organization_id_id_fk" FOREIGN KEY ("organization_id","guide_id") REFERENCES "public"."guide"("organization_id","id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "guide_owner_user_idx" ON "guide_owner" USING btree ("organization_id","user_id");--> statement-breakpoint
-- Hand-written below (drizzle-kit doesn't model these).
-- Whoever first synced a guide owns it.
INSERT INTO "guide_owner" ("organization_id", "guide_id", "user_id")
SELECT "organization_id", "id", "owner_id" FROM "guide" WHERE "owner_id" IS NOT NULL;--> statement-breakpoint
-- Guides from before locking were gated by their folder's policy; keep
-- them locked so their edits still wait for approval. New guides start unlocked.
UPDATE "guide" SET "locked" = true;
