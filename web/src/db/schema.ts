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

// Enterprise: SSO (`@better-auth/sso`), SCIM (`@better-auth/scim`), Better
// Auth's rate limits, and workspace policy. Field names follow the plugins'
// models; Better Auth checks them against these tables at startup.

/** An OIDC or SAML identity provider for one workspace's email domains. */
export const ssoProvider = pgTable(
  "sso_provider",
  {
    id: text("id").primaryKey(),
    /** Unique; used in callback URLs and the verification TXT record name. */
    providerId: text("provider_id").notNull().unique(),
    issuer: text("issuer").notNull(),
    /** Comma-separated email domains. */
    domain: text("domain").notNull(),
    domainVerified: boolean("domain_verified").default(false),
    /** JSON. Client secrets and keys are in here. */
    oidcConfig: text("oidc_config"),
    samlConfig: text("saml_config"),
    userId: text("user_id").references(() => user.id, { onDelete: "set null" }),
    organizationId: text("organization_id").references(() => organization.id, { onDelete: "cascade" }),
  },
  (t) => [index("sso_provider_organization_id_idx").on(t.organizationId)],
);

/** A SCIM connection made in the admin console; `provisioningDomainId` is the workspace. */
export const scimManagedConnection = pgTable(
  "scim_managed_connection",
  {
    id: text("id").primaryKey(),
    creationRequestId: text("creation_request_id").notNull().unique(),
    connectionId: text("connection_id").notNull().unique(),
    provisioningDomainId: text("provisioning_domain_id").notNull(),
    status: text("status").notNull(),
    revision: integer("revision").notNull(),
    createdAt: timestamp("created_at").notNull(),
    createdBy: text("created_by").notNull(),
    decommissionStartedAt: timestamp("decommission_started_at"),
    decommissionStartedBy: text("decommission_started_by"),
    decommissionedAt: timestamp("decommissioned_at"),
    decommissionedBy: text("decommissioned_by"),
  },
  (t) => [index("scim_managed_connection_domain_idx").on(t.provisioningDomainId)],
);

/** A connection's bearer token, stored only as an HMAC digest. */
export const scimManagedCredential = pgTable(
  "scim_managed_credential",
  {
    id: text("id").primaryKey(),
    connectionRecordId: text("connection_record_id")
      .notNull()
      .references(() => scimManagedConnection.id, { onDelete: "cascade" }),
    credentialId: text("credential_id").notNull().unique(),
    tokenDigest: text("token_digest").notNull(),
    hashVersion: text("hash_version").notNull(),
    activeSlotKey: text("active_slot_key").notNull().unique(),
    status: text("status").notNull(),
    serializedScopes: text("serialized_scopes").notNull(),
    expiresAt: timestamp("expires_at").notNull(),
    createdAt: timestamp("created_at").notNull(),
    createdBy: text("created_by").notNull(),
    lastUsedAt: timestamp("last_used_at"),
    revokedAt: timestamp("revoked_at"),
    revokedBy: text("revoked_by"),
    decommissionedAt: timestamp("decommissioned_at"),
  },
  (t) => [index("scim_managed_credential_connection_idx").on(t.connectionRecordId)],
);

export const scimManagedConnectionEvent = pgTable(
  "scim_managed_connection_event",
  {
    id: text("id").primaryKey(),
    connectionRecordId: text("connection_record_id")
      .notNull()
      .references(() => scimManagedConnection.id, { onDelete: "cascade" }),
    eventKey: text("event_key").notNull().unique(),
    sequence: integer("sequence").notNull(),
    type: text("type").notNull(),
    actorId: text("actor_id").notNull(),
    credentialId: text("credential_id"),
    createdAt: timestamp("created_at").notNull(),
  },
  (t) => [index("scim_managed_connection_event_connection_idx").on(t.connectionRecordId)],
);

export const scimConnectionBinding = pgTable(
  "scim_connection_binding",
  {
    id: text("id").primaryKey(),
    connectionId: text("connection_id").notNull(),
    connectionKey: text("connection_key").notNull().unique(),
    provisioningDomainId: text("provisioning_domain_id").notNull(),
    createdAt: timestamp("created_at").notNull(),
    decommissionedAt: timestamp("decommissioned_at"),
    decommissionStatus: text("decommission_status").notNull().default("active"),
    decommissionCursorUserId: text("decommission_cursor_user_id"),
    decommissionReconciledUserCount: integer("decommission_reconciled_user_count").notNull().default(0),
    decommissionBatchCount: integer("decommission_batch_count").notNull().default(0),
    decommissionRevision: integer("decommission_revision").notNull().default(0),
    decommissionCompletedAt: timestamp("decommission_completed_at"),
    decommissionLeaseId: text("decommission_lease_id"),
    decommissionLeaseExpiresAt: timestamp("decommission_lease_expires_at"),
  },
  (t) => [index("scim_connection_binding_connection_idx").on(t.connectionId)],
);

/** One Better Auth user's state across SCIM connections. */
export const scimSubject = pgTable(
  "scim_subject",
  {
    id: text("id").primaryKey(),
    userId: text("user_id")
      .notNull()
      .unique()
      .references(() => user.id, { onDelete: "cascade" }),
    profileSourceId: text("profile_source_id"),
    revision: integer("revision").notNull(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (t) => [index("scim_subject_profile_source_idx").on(t.profileSourceId)],
);

/** A User resource from one connection, linked to a Better Auth user. */
export const scimUser = pgTable(
  "scim_user",
  {
    id: text("id").primaryKey(),
    connectionId: text("connection_id").notNull(),
    provisioningDomainId: text("provisioning_domain_id").notNull(),
    userId: text("user_id")
      .notNull()
      .references(() => user.id, { onDelete: "cascade" }),
    connectionUserKey: text("connection_user_key").notNull().unique(),
    userName: text("user_name").notNull(),
    userNameKey: text("user_name_key").notNull().unique(),
    primaryEmail: text("primary_email").notNull(),
    workEmailValueIndex: text("work_email_value_index").notNull(),
    emailValueIndex: text("email_value_index").notNull(),
    displayName: text("display_name").notNull(),
    formattedName: text("formatted_name").notNull(),
    givenName: text("given_name"),
    familyName: text("family_name"),
    serializedEmails: text("serialized_emails").notNull(),
    serializedAttributes: text("serialized_attributes"),
    externalId: text("external_id"),
    externalIdKey: text("external_id_key").unique(),
    active: boolean("active").notNull(),
    orderKey: text("order_key").notNull().unique(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (t) => [
    index("scim_user_connection_idx").on(t.connectionId),
    index("scim_user_domain_idx").on(t.provisioningDomainId),
    index("scim_user_user_idx").on(t.userId),
  ],
);

export const scimIdentityTombstone = pgTable(
  "scim_identity_tombstone",
  {
    id: text("id").primaryKey(),
    connectionId: text("connection_id").notNull(),
    provisioningDomainId: text("provisioning_domain_id").notNull(),
    externalId: text("external_id").notNull(),
    externalIdKey: text("external_id_key").notNull().unique(),
    userId: text("user_id")
      .notNull()
      .references(() => user.id, { onDelete: "cascade" }),
    profile: text("profile").notNull(),
    deletedAt: timestamp("deleted_at").notNull(),
  },
  (t) => [
    index("scim_identity_tombstone_connection_idx").on(t.connectionId),
    index("scim_identity_tombstone_domain_idx").on(t.provisioningDomainId),
    index("scim_identity_tombstone_user_idx").on(t.userId),
  ],
);

export const scimGroup = pgTable(
  "scim_group",
  {
    id: text("id").primaryKey(),
    connectionId: text("connection_id").notNull(),
    provisioningDomainId: text("provisioning_domain_id").notNull(),
    revision: integer("revision").notNull().default(0),
    displayName: text("display_name").notNull(),
    displayNameKey: text("display_name_key").notNull().unique(),
    externalId: text("external_id"),
    externalIdKey: text("external_id_key").unique(),
    orderKey: text("order_key").notNull().unique(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (t) => [index("scim_group_connection_idx").on(t.connectionId), index("scim_group_domain_idx").on(t.provisioningDomainId)],
);

export const scimGroupMember = pgTable(
  "scim_group_member",
  {
    id: text("id").primaryKey(),
    connectionId: text("connection_id").notNull(),
    groupId: text("group_id")
      .notNull()
      .references(() => scimGroup.id, { onDelete: "cascade" }),
    scimUserId: text("scim_user_id")
      .notNull()
      .references(() => scimUser.id, { onDelete: "cascade" }),
    membershipKey: text("membership_key").notNull().unique(),
    createdAt: timestamp("created_at").notNull(),
  },
  (t) => [
    index("scim_group_member_connection_idx").on(t.connectionId),
    index("scim_group_member_group_idx").on(t.groupId),
    index("scim_group_member_user_idx").on(t.scimUserId),
  ],
);

/** A role a SCIM Group gives a user; the projection turns these into member roles. */
export const scimProjectionGrant = pgTable(
  "scim_projection_grant",
  {
    id: text("id").primaryKey(),
    connectionId: text("connection_id").notNull(),
    provisioningDomainId: text("provisioning_domain_id").notNull(),
    scimUserId: text("scim_user_id")
      .notNull()
      .references(() => scimUser.id, { onDelete: "cascade" }),
    userId: text("user_id")
      .notNull()
      .references(() => user.id, { onDelete: "cascade" }),
    sourceKind: text("source_kind").notNull(),
    sourceId: text("source_id").notNull(),
    sourceValue: text("source_value"),
    role: text("role").notNull(),
    grantKey: text("grant_key").notNull().unique(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (t) => [
    index("scim_projection_grant_connection_idx").on(t.connectionId),
    index("scim_projection_grant_domain_idx").on(t.provisioningDomainId),
    index("scim_projection_grant_scim_user_idx").on(t.scimUserId),
    index("scim_projection_grant_user_idx").on(t.userId),
  ],
);

/** Better Auth's rate-limit counters, shared by every app instance. */
export const rateLimit = pgTable("rate_limit", {
  id: text("id").primaryKey(),
  key: text("key").notNull().unique(),
  count: integer("count").notNull(),
  /** Milliseconds since the epoch. */
  lastRequest: bigint("last_request", { mode: "number" }).notNull(),
});

/** Policy a workspace's admins set (see lib/policy.ts). Workspaces without a row have the defaults. */
export const workspacePolicy = pgTable("workspace_policy", {
  /** The organization id again: Better Auth's adapter needs an `id`. */
  id: text("id").primaryKey(),
  organizationId: text("organization_id")
    .notNull()
    .unique()
    .references(() => organization.id, { onDelete: "cascade" }),
  /** `off`, `workspace` or `public`: the most a guide may be shared with. */
  sharing: text("sharing").notNull().default("public"),
  /** Members on a verified SSO domain must sign in with SSO (owners excepted). */
  requireSso: boolean("require_sso").notNull().default(false),
  /** The role SSO and SCIM give new members. */
  provisioningRole: text("provisioning_role").notNull().default("editor"),
  /** Days to keep revisions that are neither a head nor published; null keeps them. */
  revisionRetentionDays: integer("revision_retention_days"),
  /** Days to keep audit events; null keeps them. */
  auditRetentionDays: integer("audit_retention_days"),
  updatedAt: updatedAt(),
});
