# SOC 2 groundwork

The controls and routines a SOC 2 Type I/II audit of the web service will
look for, as they apply to GuideRecorder. It's groundwork: each section says
what exists in the product and what the team has to do and keep evidence of.
Technical details are in [web-security.md](web-security.md) and
[web-deployment.md](web-deployment.md).

## Access control

**Customers' access** (in the product): workspace roles (owner, admin,
approver, editor, viewer); SSO with domain verification and "require SSO";
SCIM so leavers lose access when the directory deactivates them; seats.

**Our access** (to production):

- Production (cloud console, database, storage, logs, Stripe) only through
  SSO with MFA, by named accounts; no shared logins. Break-glass credentials
  sealed, use alerted and reviewed.
- Least privilege: engineers read-only by default; write access granted per
  task and time-limited.
- The app's own credentials (database, S3, KMS, Stripe) live in the secret
  manager, never in the repository or images.
- Onboarding and offboarding checklists; access removed on the last day.

### Access reviews (quarterly)

1. Export who has access to: the cloud account(s), the database, the
   storage bucket and KMS key, the log store, Stripe, GitHub (admins and
   people who can merge to `main` or create releases), and the secret
   manager.
2. The engineering lead confirms each person still needs each access; remove
   the rest within 5 working days.
3. Check service credentials: last rotated (see the rotation runbook),
   still used.
4. Save the export, the decisions and the tickets for removals as evidence.

Enterprise customers review their own members in Admin.

## Logging and monitoring

- Application logs to stdout, shipped to the log store, kept 1 year,
  readable only by engineering, not editable by them.
- **Security events** (`web/src/lib/security-events.ts`) are structured JSON
  lines: member role changes and removals, SSO and SCIM provisioning and
  deprovisioning, policy changes, SSO provider and domain changes, SCIM
  connection and token changes, blocked sign-ins, rate limits hit and
  retention runs. Each has the workspace, actor, target, time and details.
  The audit log (#51) plugs in with `setSecurityEventSink` to also store them
  as tamper-evident audit events customers can see.
- Infrastructure: cloud audit logs (CloudTrail or equivalent) on, kept 1
  year; database and KMS access logged.
- Alerts (see the deployment doc) go to the on-call rotation.

## Change management

- All changes through pull requests to `main` with CI (tests, typecheck) and
  a review; branch protection forbids direct pushes.
- Migrations are reviewed with the code and applied by the deploy pipeline.
- Deploys are logged (who, what commit, when); rollback is redeploying the
  previous image.

## Data retention and deletion

- Trash purged after 30 days; old revisions and audit events per the
  workspace's retention settings; unreferenced images deleted; expired
  sessions and tokens deleted (hourly job, `web/src/lib/retention.ts`).
- Deleting a workspace deletes its rows (database cascades). Its images
  under `orgs/<workspace id>/` in the bucket have to be deleted too; doing
  that automatically is a follow-up.
- Backups expire on their schedule (6 months for monthly dumps), so deleted
  data is gone from backups within that time.

## Availability and backups

- Health checks and alerts; at least two app instances; managed Postgres
  with point-in-time recovery; bucket versioning.
- Quarterly restore drill (deployment doc), with the results kept.

## Vendors

Keep a list with what each processes and its SOC 2 report or equivalent:
cloud provider, Postgres host, Cloudflare (R2), Stripe, Resend, the log
store. Review yearly.

## Incident response runbook

**Severity.** SEV1: customer data exposed or service down for everyone.
SEV2: a security control broken or a feature down for many. SEV3: limited
impact.

1. **Detect and declare.** Anyone can declare an incident from an alert, a
   report (security@ address) or something they notice. Open an incident
   channel and ticket; name an incident lead.
2. **Contain.**
   - Leaked credential: rotate it now (key rotation runbook), skipping the
     overlap period.
   - Compromised account: delete its sessions (`delete from session where
     user_id = …`), reset its password, remove it from workspaces.
   - Abuse from a client: block it at the proxy; tighten rate limits.
   - Compromised SCIM token or IdP: the customer's admin (or we, on their
     request) revokes the token or removes the SSO provider.
   - Bad deploy: roll back to the previous image.
3. **Investigate.** Preserve logs and snapshots before changing more.
   Establish the timeline, what data and which workspaces were affected,
   using security events, application logs and cloud audit logs.
4. **Notify.** If customer data was exposed: tell affected customers within
   72 hours of confirming it, with what happened, what data, what we did and
   what they should do; meet GDPR and contractual duties (DPA). The incident
   lead owns the message; leadership approves it.
5. **Recover.** Restore from backup if data was lost or altered; verify;
   watch closely for a day.
6. **Review.** Blameless write-up within 5 working days: timeline, root
   cause, what went well and badly, follow-ups with owners and dates. Keep
   it as evidence; review open follow-ups weekly.

Test the runbook once a year with a tabletop exercise.
