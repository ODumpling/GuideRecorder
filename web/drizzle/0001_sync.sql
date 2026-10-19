CREATE SEQUENCE "public"."sync_seq" INCREMENT BY 1 MINVALUE 1 MAXVALUE 9223372036854775807 START WITH 1 CACHE 1;--> statement-breakpoint
CREATE TABLE "device_code" (
	"id" text PRIMARY KEY NOT NULL,
	"device_code" text NOT NULL,
	"user_code" text NOT NULL,
	"user_id" text,
	"expires_at" timestamp NOT NULL,
	"status" text NOT NULL,
	"last_polled_at" timestamp,
	"polling_interval" integer,
	"client_id" text,
	"scope" text,
	CONSTRAINT "device_code_device_code_unique" UNIQUE("device_code"),
	CONSTRAINT "device_code_user_code_unique" UNIQUE("user_code")
);
--> statement-breakpoint
CREATE TABLE "guide" (
	"organization_id" text NOT NULL,
	"id" text NOT NULL,
	"owner_id" text,
	"visibility" text DEFAULT 'workspace' NOT NULL,
	"head_revision_id" text,
	"published_revision_id" text,
	"seq" bigint DEFAULT nextval('sync_seq') NOT NULL,
	"deleted_at" timestamp,
	"created_at" timestamp DEFAULT now() NOT NULL,
	"updated_at" timestamp DEFAULT now() NOT NULL,
	CONSTRAINT "guide_organization_id_id_pk" PRIMARY KEY("organization_id","id")
);
--> statement-breakpoint
CREATE TABLE "revision" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"guide_id" text NOT NULL,
	"parent_id" text,
	"guide_json" jsonb NOT NULL,
	"author_id" text,
	"source" text NOT NULL,
	"created_at" timestamp DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "revision_asset" (
	"revision_id" text NOT NULL,
	"path" text NOT NULL,
	"organization_id" text NOT NULL,
	"sha256" text NOT NULL,
	CONSTRAINT "revision_asset_revision_id_path_pk" PRIMARY KEY("revision_id","path")
);
--> statement-breakpoint
ALTER TABLE "device_code" ADD CONSTRAINT "device_code_user_id_user_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."user"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "guide" ADD CONSTRAINT "guide_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "guide" ADD CONSTRAINT "guide_owner_id_user_id_fk" FOREIGN KEY ("owner_id") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "revision" ADD CONSTRAINT "revision_author_id_user_id_fk" FOREIGN KEY ("author_id") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "revision" ADD CONSTRAINT "revision_organization_id_guide_id_guide_organization_id_id_fk" FOREIGN KEY ("organization_id","guide_id") REFERENCES "public"."guide"("organization_id","id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "revision_asset" ADD CONSTRAINT "revision_asset_revision_id_revision_id_fk" FOREIGN KEY ("revision_id") REFERENCES "public"."revision"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "revision_asset" ADD CONSTRAINT "revision_asset_organization_id_sha256_asset_organization_id_sha256_fk" FOREIGN KEY ("organization_id","sha256") REFERENCES "public"."asset"("organization_id","sha256") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "guide_organization_seq_idx" ON "guide" USING btree ("organization_id","seq");--> statement-breakpoint
CREATE INDEX "revision_guide_idx" ON "revision" USING btree ("organization_id","guide_id");