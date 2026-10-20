CREATE TABLE "platform_settings" (
	"id" text PRIMARY KEY NOT NULL,
	"registration" boolean DEFAULT true NOT NULL,
	"disabled_plans" text[] DEFAULT '{}'::text[] NOT NULL,
	"updated_by" text,
	"updated_at" timestamp DEFAULT now() NOT NULL
);
