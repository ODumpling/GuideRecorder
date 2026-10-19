"use client";

import { useState } from "react";
import { authClient } from "@/lib/auth-client";
import type { PlanName } from "@/lib/plans";
import { Button, Card, ErrorText } from "../ui";

interface Props {
  workspaceId: string;
  plan: PlanName;
  /** Stripe is configured. */
  enabled: boolean;
  canManage: boolean;
}

export function Billing({ workspaceId, plan, enabled, canManage }: Props) {
  const [error, setError] = useState<string | null>(null);
  if (!enabled) return null;

  const returnUrl = () => `${window.location.origin}/dashboard`;

  async function upgrade(target: "pro" | "enterprise") {
    setError(null);
    const { error } = await authClient.subscription.upgrade({
      plan: target,
      referenceId: workspaceId,
      customerType: "organization",
      successUrl: returnUrl(),
      cancelUrl: returnUrl(),
    });
    if (error) setError(error.message ?? "Couldn't start checkout.");
  }

  async function portal() {
    setError(null);
    const { error } = await authClient.subscription.billingPortal({ referenceId: workspaceId, customerType: "organization", returnUrl: returnUrl() });
    if (error) setError(error.message ?? "Couldn't open billing.");
  }

  return (
    <Card>
      <h2 className="mb-2 text-lg font-semibold">Billing</h2>
      {!canManage ? (
        <p className="text-sm text-muted-foreground">Only the workspace owner manages billing.</p>
      ) : (
        <div className="flex flex-wrap gap-2">
          {plan === "free" && <Button onClick={() => upgrade("pro")}>Upgrade to Pro</Button>}
          {plan !== "enterprise" && (
            <Button variant={plan === "free" ? "secondary" : "primary"} onClick={() => upgrade("enterprise")}>
              Upgrade to Enterprise
            </Button>
          )}
          {plan !== "free" && (
            <Button variant="secondary" onClick={portal}>
              Manage billing
            </Button>
          )}
        </div>
      )}
      <div className="mt-2">
        <ErrorText>{error}</ErrorText>
      </div>
    </Card>
  );
}
