import { useState } from "react";
import type { PaidPlan, PlatformSettings as Settings } from "@/lib/platform";
import { savePlatform } from "@/server/staff";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Switch } from "@guiderecorder/ui/switch";
import { ErrorText } from "@/components/ui";
import { useAction } from "@/components/admin/useAction";

const PLAN_LABEL: Record<PaidPlan, string> = { pro: "Pro", enterprise: "Enterprise" };

interface Plan {
  plan: PaidPlan;
  /** Has a Stripe price, so it can be bought at all. */
  priced: boolean;
  /** Active subscriptions. */
  subscriptions: number;
}

/** Sign-ups and which plans can be bought, for the whole service. */
export function PlatformSettings({ settings, plans, billing }: { settings: Settings; plans: Plan[]; billing: boolean }) {
  const { run, error, busy } = useAction();
  const [draft, setDraft] = useState(settings);
  const changed = JSON.stringify(draft) !== JSON.stringify(settings);
  const setPlan = (plan: PaidPlan, on: boolean) =>
    setDraft({ ...draft, disabledPlans: on ? draft.disabledPlans.filter((p) => p !== plan) : [...draft.disabledPlans, plan] });

  return (
    <>
      <Card className="grid gap-4 p-6">
        <h2 className="text-lg font-semibold">Sign-ups</h2>
        <Toggle
          id="registration"
          label="Anyone can create an account"
          hint="When off, only people with a pending invitation, and users from a workspace's SSO or SCIM, can create one. Existing users can still sign in."
          checked={draft.registration}
          onChange={(registration) => setDraft({ ...draft, registration })}
        />
      </Card>

      <Card className="grid gap-4 p-6">
        <div>
          <h2 className="text-lg font-semibold">Plans</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Workspaces can only buy plans that are on. Turning one off keeps existing subscriptions and licence grants working.
            {!billing && " Billing isn't configured, so no plans can be bought."}
          </p>
        </div>
        {plans.map((p) => (
          <Toggle
            key={p.plan}
            id={`plan-${p.plan}`}
            label={PLAN_LABEL[p.plan]}
            hint={`${p.subscriptions} active subscription${p.subscriptions === 1 ? "" : "s"}${p.priced ? "" : " · no Stripe price set"}`}
            checked={!draft.disabledPlans.includes(p.plan)}
            onChange={(on) => setPlan(p.plan, on)}
          />
        ))}
      </Card>

      <div className="grid gap-2">
        <Button variant="brand" size="lg" className="justify-self-start" disabled={!changed || busy} onClick={() => run(() => savePlatform({ data: draft }))}>
          {busy ? "Saving…" : "Save"}
        </Button>
        <ErrorText>{error}</ErrorText>
      </div>
    </>
  );
}

function Toggle({ id, label, hint, checked, onChange }: { id: string; label: string; hint: string; checked: boolean; onChange: (on: boolean) => void }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <label htmlFor={id} className="grid gap-0.5">
        <span className="text-sm font-medium">{label}</span>
        <span className="text-xs text-muted-foreground">{hint}</span>
      </label>
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}
