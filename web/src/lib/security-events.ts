// Security-relevant events (admin changes, SSO and SCIM, rate limits,
// retention runs), as one structured log line each. The audit log (#51)
// plugs in with `setSecurityEventSink` to also store them as audit events;
// until then they go to stdout as JSON for the log pipeline to keep.

export type SecurityEventType =
  | "member.role_changed"
  | "member.removed"
  | "member.provisioned"
  | "member.deprovisioned"
  | "policy.updated"
  | "sso.provider_registered"
  | "sso.provider_deleted"
  | "sso.domain_verification_requested"
  | "sso.domain_verified"
  | "sso.sign_in_blocked"
  | "scim.connection_created"
  | "scim.credential_rotated"
  | "scim.credential_revoked"
  | "scim.connection_decommissioned"
  | "scim.request_blocked"
  | "rate_limit.exceeded"
  | "retention.run";

export interface SecurityEvent {
  type: SecurityEventType;
  /** The workspace it happened in, if any. */
  organizationId?: string | null;
  /** Who did it: a user id, `scim:<connection>`, or `system`. */
  actorId?: string | null;
  /** What it was done to, e.g. a member's user id or a provider id. */
  targetId?: string | null;
  ip?: string | null;
  details?: Record<string, unknown>;
}

export interface SecurityEventSink {
  record(event: SecurityEvent & { at: Date }): void | Promise<void>;
}

export const consoleSink: SecurityEventSink = {
  record(event) {
    console.info(JSON.stringify({ level: "info", msg: "security_event", ...event, at: event.at.toISOString() }));
  },
};

let sink: SecurityEventSink = consoleSink;

/** Replaces where events go, e.g. with one that also writes audit events. Returns the previous sink. */
export function setSecurityEventSink(next: SecurityEventSink): SecurityEventSink {
  const previous = sink;
  sink = next;
  return previous;
}

/** Records an event. Never throws: a failing sink must not fail the request. */
export async function securityEvent(event: SecurityEvent, at = new Date()): Promise<void> {
  try {
    await sink.record({ ...event, at });
  } catch (error) {
    console.error("security event sink failed", error);
  }
}
