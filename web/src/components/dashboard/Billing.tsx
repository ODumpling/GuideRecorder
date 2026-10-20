import { useState } from "react";
import { Check } from "lucide-react";
import { authClient } from "@/lib/auth-client";
import type { PaidPlan } from "@/lib/platform";
import { ENTITLEMENTS, type PlanName } from "@/lib/plans";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { cn } from "@guiderecorder/ui/utils";
import { ErrorText } from "@/components/ui";

interface Props {
  workspaceId: string;
  plan: PlanName;
  /** Stripe is configured. */
  enabled: boolean;
  canManage: boolean;
  /** The paid plans that can be bought now; staff can turn plans off. */
  offered: PaidPlan[];
}

const PLANS: PlanName[] = ["free", "pro", "enterprise"];
const RANK: Record<PlanName, number> = { free: 0, pro: 1, enterprise: 2 };
const LABEL: Record<PlanName, string> = { free: "Free", pro: "Pro", enterprise: "Enterprise" };

const GB = 1024 ** 3;

/** What each plan gives, from its entitlements. */
function features(plan: PlanName): string[] {
  const e = ENTITLEMENTS[plan];
  const seats = e.seats === 1 ? "1 seat" : `Up to ${e.seats.toLocaleString()} seats`;
  const list = [seats];
  if (e.storageBytes) list.push(`${e.storageBytes >= 1024 * GB ? `${e.storageBytes / (1024 * GB)} TB` : `${e.storageBytes / GB} GB`} storage`);
  if (e.sync) list.push("Desktop sync and cloud library");
  if (e.sharing) list.push("Share links");
  if (e.webEditor) list.push("Edit guides on the web");
  if (e.approvals) list.push("Review and approvals");
  if (e.audit) list.push("Audit log");
  if (e.sso) list.push("SSO, SCIM and admin policies");
  if (plan === "free") list.push("Sign in and manage members");
  return list;
}

/** The workspace's plan, with upgrades and downgrades through Stripe. */
export function Billing({ workspaceId, plan, enabled, canManage, offered }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<PlanName | "portal" | null>(null);

  const returnUrl = () => `${window.location.origin}/dashboard`;

  async function run(key: PlanName | "portal", action: () => Promise<{ error: { message?: string } | null }>, fallback: string) {
    setError(null);
    setBusy(key);
    const { error } = await action();
    setBusy(null);
    if (error) setError(error.message ?? fallback);
  }

  // Switching between paid plans changes the current subscription; Stripe
  // confirms the change (and prorates) before it applies.
  const change = (target: "pro" | "enterprise") =>
    run(
      target,
      () =>
        authClient.subscription.upgrade({
          plan: target,
          referenceId: workspaceId,
          customerType: "organization",
          successUrl: returnUrl(),
          cancelUrl: returnUrl(),
          returnUrl: returnUrl(),
        }),
      "Couldn't start checkout.",
    );

  // Back to Free: cancels the subscription at the end of the period.
  const cancel = () =>
    run(
      "free",
      () => authClient.subscription.cancel({ referenceId: workspaceId, customerType: "organization", returnUrl: returnUrl() }),
      "Couldn't cancel the subscription.",
    );

  const portal = () =>
    run(
      "portal",
      () => authClient.subscription.billingPortal({ referenceId: workspaceId, customerType: "organization", returnUrl: returnUrl() }),
      "Couldn't open billing.",
    );

  return (
    <Card id="plan" className="block scroll-mt-6 p-6">
      <div className="mb-4 flex flex-wrap items-start justify-between gap-2">
        <div>
          <h2 className="text-lg font-semibold">Plan</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            {!enabled
              ? "Billing isn't set up on this server."
              : canManage
                ? "Upgrade or downgrade this workspace's plan."
                : "Only the workspace owner changes the plan."}
          </p>
        </div>
        {enabled && canManage && plan !== "free" ? (
          <Button variant="outline" size="sm" disabled={busy !== null} onClick={portal}>
            Invoices and payment
          </Button>
        ) : null}
      </div>
      <ul className="grid gap-3 sm:grid-cols-3">
        {PLANS.map((p) => {
          const current = p === plan;
          const up = RANK[p] > RANK[plan];
          const available = p === "free" || offered.includes(p);
          return (
            <li
              key={p}
              className={cn("flex flex-col gap-3 rounded-xl border border-border p-4", current && "border-brand ring-1 ring-brand")}
            >
              <div className="flex items-center justify-between">
                <span className="font-semibold">{LABEL[p]}</span>
                {current ? <span className="text-xs font-medium text-brand">Current plan</span> : null}
              </div>
              <ul className="flex flex-1 flex-col gap-1.5 text-sm text-muted-foreground">
                {features(p).map((f) => (
                  <li key={f} className="flex gap-1.5">
                    <Check className="mt-0.5 size-3.5 shrink-0 text-brand" />
                    {f}
                  </li>
                ))}
              </ul>
              {enabled && canManage && !current && !available ? (
                <p className="text-sm text-muted-foreground">Not available right now.</p>
              ) : null}
              {enabled && canManage && !current && available ? (
                <Button
                  variant={up ? "brand" : "outline"}
                  disabled={busy !== null}
                  onClick={() => (p === "free" ? cancel() : change(p))}
                >
                  {busy === p ? "Opening…" : `${up ? "Upgrade" : "Downgrade"} to ${LABEL[p]}`}
                </Button>
              ) : null}
            </li>
          );
        })}
      </ul>
      <div className="mt-2">
        <ErrorText>{error}</ErrorText>
      </div>
    </Card>
  );
}
