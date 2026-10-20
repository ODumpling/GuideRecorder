CREATE TABLE "approval_policy" (
	"organization_id" text NOT NULL,
	"folder" text DEFAULT '' NOT NULL,
	"required_approvals" integer DEFAULT 1 NOT NULL,
	"approver_ids" text[] DEFAULT '{}'::text[] NOT NULL,
	"allow_self_approval" boolean DEFAULT false NOT NULL,
	"reset_on_new_revision" boolean DEFAULT true NOT NULL,
	"updated_by" text,
	"updated_at" timestamp DEFAULT now() NOT NULL,
	CONSTRAINT "approval_policy_organization_id_folder_pk" PRIMARY KEY("organization_id","folder")
);
--> statement-breakpoint
CREATE TABLE "audit_event" (
	"organization_id" text NOT NULL,
	"seq" bigint NOT NULL,
	"actor_id" text,
	"action" text NOT NULL,
	"target_type" text NOT NULL,
	"target_id" text,
	"before_hash" text,
	"after_hash" text,
	"ip" text,
	"user_agent" text,
	"metadata" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"prev_hash" text NOT NULL,
	"hash" text NOT NULL,
	"created_at" timestamp NOT NULL,
	CONSTRAINT "audit_event_organization_id_seq_pk" PRIMARY KEY("organization_id","seq")
);
--> statement-breakpoint
CREATE TABLE "audit_settings" (
	"organization_id" text PRIMARY KEY NOT NULL,
	"retention_days" integer,
	"updated_at" timestamp DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "change_request" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"guide_id" text NOT NULL,
	"revision_id" text NOT NULL,
	"base_revision_id" text,
	"status" text DEFAULT 'open' NOT NULL,
	"author_id" text,
	"note" text DEFAULT '' NOT NULL,
	"published_revision_id" text,
	"closed_by" text,
	"closed_at" timestamp,
	"created_at" timestamp DEFAULT now() NOT NULL,
	"updated_at" timestamp DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "review" (
	"id" text PRIMARY KEY NOT NULL,
	"change_request_id" text NOT NULL,
	"reviewer_id" text,
	"revision_id" text NOT NULL,
	"verdict" text NOT NULL,
	"body" text DEFAULT '' NOT NULL,
	"item_id" text,
	"created_at" timestamp DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "approval_policy" ADD CONSTRAINT "approval_policy_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "approval_policy" ADD CONSTRAINT "approval_policy_updated_by_user_id_fk" FOREIGN KEY ("updated_by") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "audit_settings" ADD CONSTRAINT "audit_settings_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "change_request" ADD CONSTRAINT "change_request_revision_id_revision_id_fk" FOREIGN KEY ("revision_id") REFERENCES "public"."revision"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "change_request" ADD CONSTRAINT "change_request_author_id_user_id_fk" FOREIGN KEY ("author_id") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "change_request" ADD CONSTRAINT "change_request_closed_by_user_id_fk" FOREIGN KEY ("closed_by") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "change_request" ADD CONSTRAINT "change_request_organization_id_guide_id_guide_organization_id_id_fk" FOREIGN KEY ("organization_id","guide_id") REFERENCES "public"."guide"("organization_id","id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "review" ADD CONSTRAINT "review_change_request_id_change_request_id_fk" FOREIGN KEY ("change_request_id") REFERENCES "public"."change_request"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "review" ADD CONSTRAINT "review_reviewer_id_user_id_fk" FOREIGN KEY ("reviewer_id") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "audit_event_created_idx" ON "audit_event" USING btree ("organization_id","created_at");--> statement-breakpoint
CREATE INDEX "audit_event_action_idx" ON "audit_event" USING btree ("organization_id","action");--> statement-breakpoint
CREATE INDEX "change_request_guide_idx" ON "change_request" USING btree ("organization_id","guide_id");--> statement-breakpoint
CREATE UNIQUE INDEX "change_request_open_idx" ON "change_request" USING btree ("organization_id","guide_id") WHERE status = 'open';--> statement-breakpoint
CREATE INDEX "review_change_request_idx" ON "review" USING btree ("change_request_id");--> statement-breakpoint
-- Hand-written below (drizzle-kit doesn't model these).
-- Guides synced before approvals were live: their head is what readers saw.
UPDATE "guide" SET "published_revision_id" = "head_revision_id" WHERE "published_revision_id" IS NULL;--> statement-breakpoint
-- The audit log is append-only. Retention pruning deletes with
-- guiderecorder.audit_prune set for its transaction (lib/audit.ts).
CREATE FUNCTION "audit_event_append_only"() RETURNS trigger AS $$
BEGIN
	IF TG_OP = 'DELETE' AND current_setting('guiderecorder.audit_prune', true) = 'on' THEN
		RETURN OLD;
	END IF;
	RAISE EXCEPTION 'audit_event is append-only';
END
$$ LANGUAGE plpgsql;--> statement-breakpoint
CREATE TRIGGER "audit_event_append_only" BEFORE UPDATE OR DELETE ON "audit_event" FOR EACH ROW EXECUTE FUNCTION "audit_event_append_only"();