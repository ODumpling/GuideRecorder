CREATE TABLE "share_link" (
	"id" text PRIMARY KEY NOT NULL,
	"token" text NOT NULL,
	"organization_id" text NOT NULL,
	"guide_id" text NOT NULL,
	"revision_id" text,
	"expires_at" timestamp,
	"password_hash" text,
	"allow_download" boolean DEFAULT false NOT NULL,
	"views" integer DEFAULT 0 NOT NULL,
	"last_viewed_at" timestamp,
	"created_by" text,
	"revoked_at" timestamp,
	"created_at" timestamp DEFAULT now() NOT NULL,
	CONSTRAINT "share_link_token_unique" UNIQUE("token")
);
--> statement-breakpoint
CREATE TABLE "workspace_settings" (
	"organization_id" text PRIMARY KEY NOT NULL,
	"share_links" text DEFAULT 'anyone' NOT NULL,
	"share_link_max_days" integer,
	"updated_at" timestamp DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "share_link" ADD CONSTRAINT "share_link_revision_id_revision_id_fk" FOREIGN KEY ("revision_id") REFERENCES "public"."revision"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "share_link" ADD CONSTRAINT "share_link_created_by_user_id_fk" FOREIGN KEY ("created_by") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "share_link" ADD CONSTRAINT "share_link_organization_id_guide_id_guide_organization_id_id_fk" FOREIGN KEY ("organization_id","guide_id") REFERENCES "public"."guide"("organization_id","id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "workspace_settings" ADD CONSTRAINT "workspace_settings_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "share_link_guide_idx" ON "share_link" USING btree ("organization_id","guide_id");