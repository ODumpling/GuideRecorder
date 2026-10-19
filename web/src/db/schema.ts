import { sql } from "drizzle-orm";
import {
  bigint,
  boolean,
  foreignKey,
  index,
  integer,
  jsonb,
  pgSequence,
  pgTable,
  primaryKey,
  text,
  timestamp,
  uniqueIndex,
} from "drizzle-orm/pg-core";

// Better Auth tables (core, organization and stripe plugins). Field names
// follow Better Auth's models; `auth.test.ts` checks them against the config.

const createdAt = () => timestamp("created_at").notNull().defaultNow();
const updatedAt = () =>
  timestamp("updated_at")
    .notNull()
    .defaultNow()
    .$onUpdate(() => new Date());

export const user = pgTable("user", {
  id: text("id").primaryKey(),
  name: text("name").notNull(),
  email: text("email").notNull().unique(),
  emailVerified: boolean("email_verified").notNull().default(false),
  image: text("image"),
  stripeCustomerId: text("stripe_customer_id"),
  createdAt: createdAt(),
  updatedAt: updatedAt(),
});

export const session = pgTable(
  "session",
  {
    id: text("id").primaryKey(),
    expiresAt: timestamp("expires_at").notNull(),
    token: text("token").notNull().unique(),
    ipAddress: text("ip_address"),
    userAgent: text("user_agent"),
    userId: text("user_id")
      .notNull()
      .references(() => user.id, { onDelete: "cascade" }),
    activeOrganizationId: text("active_organization_id"),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [index("session_user_id_idx").on(t.userId)],
);

export const account = pgTable(
  "account",
  {
    id: text("id").primaryKey(),
    accountId: text("account_id").notNull(),
    providerId: text("provider_id").notNull(),
    userId: text("user_id")
      .notNull()
      .references(() => user.id, { onDelete: "cascade" }),
    accessToken: text("access_token"),
    refreshToken: text("refresh_token"),
    idToken: text("id_token"),
    accessTokenExpiresAt: timestamp("access_token_expires_at"),
    refreshTokenExpiresAt: timestamp("refresh_token_expires_at"),
    scope: text("scope"),
    password: text("password"),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [index("account_user_id_idx").on(t.userId)],
);

export const verification = pgTable(
  "verification",
  {
    id: text("id").primaryKey(),
    identifier: text("identifier").notNull(),
    value: text("value").notNull(),
    expiresAt: timestamp("expires_at").notNull(),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [index("verification_identifier_idx").on(t.identifier)],
);

export const organization = pgTable("organization", {
  id: text("id").primaryKey(),
  name: text("name").notNull(),
  slug: text("slug").notNull().unique(),
  logo: text("logo"),
  metadata: text("metadata"),
  stripeCustomerId: text("stripe_customer_id"),
  createdAt: timestamp("created_at").notNull(),
});

export const member = pgTable(
  "member",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    userId: text("user_id")
      .notNull()
      .references(() => user.id, { onDelete: "cascade" }),
    role: text("role").notNull().default("member"),
    createdAt: timestamp("created_at").notNull(),
  },
  (t) => [
    index("member_organization_id_idx").on(t.organizationId),
    uniqueIndex("member_organization_user_idx").on(t.organizationId, t.userId),
  ],
);

export const invitation = pgTable(
  "invitation",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    email: text("email").notNull(),
    role: text("role"),
    status: text("status").notNull().default("pending"),
    expiresAt: timestamp("expires_at").notNull(),
    inviterId: text("inviter_id")
      .notNull()
      .references(() => user.id, { onDelete: "cascade" }),
    createdAt: createdAt(),
  },
  (t) => [index("invitation_organization_id_idx").on(t.organizationId)],
);

export const subscription = pgTable(
  "subscription",
  {
    id: text("id").primaryKey(),
    plan: text("plan").notNull(),
    /** The organization the subscription pays for. */
    referenceId: text("reference_id").notNull(),
    stripeCustomerId: text("stripe_customer_id"),
    stripeSubscriptionId: text("stripe_subscription_id"),
    status: text("status").default("incomplete"),
    periodStart: timestamp("period_start"),
    periodEnd: timestamp("period_end"),
    trialStart: timestamp("trial_start"),
    trialEnd: timestamp("trial_end"),
    cancelAtPeriodEnd: boolean("cancel_at_period_end").default(false),
    cancelAt: timestamp("cancel_at"),
    canceledAt: timestamp("canceled_at"),
    endedAt: timestamp("ended_at"),
    seats: integer("seats"),
    billingInterval: text("billing_interval"),
    stripeScheduleId: text("stripe_schedule_id"),
  },
  (t) => [index("subscription_reference_id_idx").on(t.referenceId)],
);

/** Device authorization (RFC 8628): how the desktop app signs in. */
export const deviceCode = pgTable("device_code", {
  id: text("id").primaryKey(),
  deviceCode: text("device_code").notNull().unique(),
  userCode: text("user_code").notNull().unique(),
  userId: text("user_id").references(() => user.id, { onDelete: "cascade" }),
  expiresAt: timestamp("expires_at").notNull(),
  status: text("status").notNull(),
  lastPolledAt: timestamp("last_polled_at"),
  pollingInterval: integer("polling_interval"),
  clientId: text("client_id"),
  scope: text("scope"),
});

// GuideRecorder tables.

/**
 * An uploaded image, content-addressed by SHA-256 within its workspace.
 * Stored at `orgs/<organizationId>/assets/<sha256>`. Scoped per workspace so
 * one workspace can't learn whether another holds a given image.
 */
export const asset = pgTable(
  "asset",
  {
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    /** Lowercase hex. */
    sha256: text("sha256").notNull(),
    size: bigint("size", { mode: "number" }).notNull(),
    contentType: text("content_type").notNull(),
    uploadedBy: text("uploaded_by").references(() => user.id, { onDelete: "set null" }),
    createdAt: createdAt(),
  },
  (t) => [primaryKey({ columns: [t.organizationId, t.sha256] })],
);

/** Orders guide changes for the sync feed; a guide takes a new value on every change. */
export const syncSeq = pgSequence("sync_seq");

/**
 * A synced guide. `id` is the guide's ULID from guide.json. The content is
 * in its revisions; `headRevisionId` is the latest. Deleted guides stay as
 * tombstones: in the trash for 30 days, then purged of their revisions.
 */
export const guide = pgTable(
  "guide",
  {
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    id: text("id").notNull(),
    ownerId: text("owner_id").references(() => user.id, { onDelete: "set null" }),
    /** `workspace`: every member sees it; `private`: only its owner. */
    visibility: text("visibility").notNull().default("workspace"),
    headRevisionId: text("head_revision_id"),
    publishedRevisionId: text("published_revision_id"),
    /** Position in the workspace's change feed. */
    seq: bigint("seq", { mode: "bigint" })
      .notNull()
      .default(sql`nextval('sync_seq')`),
    deletedAt: timestamp("deleted_at"),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [primaryKey({ columns: [t.organizationId, t.id] }), index("guide_organization_seq_idx").on(t.organizationId, t.seq)],
);

/** An immutable version of a guide: the whole guide.json. */
export const revision = pgTable(
  "revision",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id").notNull(),
    guideId: text("guide_id").notNull(),
    parentId: text("parent_id"),
    guideJson: jsonb("guide_json").notNull(),
    authorId: text("author_id").references(() => user.id, { onDelete: "set null" }),
    /** Where the change was made: `app` or `web`. */
    source: text("source").notNull(),
    createdAt: createdAt(),
  },
  (t) => [
    foreignKey({ columns: [t.organizationId, t.guideId], foreignColumns: [guide.organizationId, guide.id] }).onDelete("cascade"),
    index("revision_guide_idx").on(t.organizationId, t.guideId),
  ],
);

/** The images a revision names: bundle-relative path → committed asset. */
export const revisionAsset = pgTable(
  "revision_asset",
  {
    revisionId: text("revision_id")
      .notNull()
      .references(() => revision.id, { onDelete: "cascade" }),
    path: text("path").notNull(),
    organizationId: text("organization_id").notNull(),
    sha256: text("sha256").notNull(),
  },
  (t) => [
    primaryKey({ columns: [t.revisionId, t.path] }),
    foreignKey({ columns: [t.organizationId, t.sha256], foreignColumns: [asset.organizationId, asset.sha256] }),
  ],
);
