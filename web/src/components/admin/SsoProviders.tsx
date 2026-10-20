import { useState, type FormEvent } from "react";
import { addSsoProvider, checkDomain, getDomainToken, removeSsoProvider } from "@/server/admin";
import { NativeSelect } from "@guiderecorder/ui/native-select";
import { Textarea } from "@guiderecorder/ui/textarea";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { ErrorText, Field } from "@/components/ui";
import { useAction } from "./useAction";

interface Provider {
  providerId: string;
  type: "oidc" | "saml";
  issuer: string;
  domains: string[];
  domainVerified: boolean;
  txtRecords: string[];
}

/** The workspace's identity providers, their DNS verification, and a form to add one. */
export function SsoProviders({ providers, baseURL }: { providers: Provider[]; baseURL: string }) {
  const { run, error, busy } = useAction();
  const [type, setType] = useState<"oidc" | "saml">("oidc");
  const [tokens, setTokens] = useState<Record<string, string>>({});

  async function showToken(providerId: string) {
    const result = await run(() => getDomainToken({ data: { providerId } }));
    if (result) setTokens((t) => ({ ...t, [providerId]: result.token }));
  }

  function add(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const f = Object.fromEntries(new FormData(form)) as Record<string, string>;
    const data =
      type === "oidc"
        ? { type, providerId: f.providerId!, domain: f.domain!, issuer: f.issuer!, clientId: f.clientId!, clientSecret: f.clientSecret!, discoveryEndpoint: f.discoveryEndpoint || undefined }
        : { type, providerId: f.providerId!, domain: f.domain!, issuer: f.issuer!, entryPoint: f.entryPoint!, cert: f.cert! };
    void run(() => addSsoProvider({ data })).then((result) => {
      if (!result) return;
      form.reset();
      setTokens((t) => ({ ...t, [result.providerId]: result.txtRecords[0]?.value ?? "" }));
    });
  }

  return (
    <Card className="p-6 grid gap-4">
      <div>
        <h2 className="text-lg font-semibold">Single sign-on</h2>
        <p className="text-sm text-muted-foreground">
          Add your identity provider, then prove you own its domains with a DNS TXT record. People with an email on a verified domain join this
          workspace when they sign in.
        </p>
      </div>

      {providers.map((p) => (
        <div key={p.providerId} className="grid gap-2 rounded-lg border border-border p-4 text-sm">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>
              <span className="font-medium">{p.providerId}</span> <span className="uppercase text-muted-foreground">{p.type}</span>{" "}
              <span className="text-muted-foreground">{p.domains.join(", ")}</span>
            </span>
            <span className="flex gap-2">
              {p.domainVerified ? (
                <span className="text-success">Verified</span>
              ) : (
                <>
                  <Button variant="outline" disabled={busy} onClick={() => showToken(p.providerId)}>
                    Show DNS record
                  </Button>
                  <Button variant="brand" disabled={busy} onClick={() => run(() => checkDomain({ data: { providerId: p.providerId } }))}>
                    Verify
                  </Button>
                </>
              )}
              <Button variant="destructive" disabled={busy} onClick={() => run(() => removeSsoProvider({ data: { providerId: p.providerId } }))}>
                Remove
              </Button>
            </span>
          </div>
          {!p.domainVerified && tokens[p.providerId] && (
            <div className="grid gap-1 rounded-md bg-muted p-3 font-mono text-xs">
              {p.txtRecords.map((name) => (
                <span key={name}>
                  {name} TXT "{tokens[p.providerId]}"
                </span>
              ))}
            </div>
          )}
          <p className="text-xs text-muted-foreground">
            {p.type === "oidc" ? (
              <>Redirect URI: {baseURL}/api/auth/sso/callback/{p.providerId}</>
            ) : (
              <>
                ACS URL: {baseURL}/api/auth/sso/saml2/sp/acs/{p.providerId} · SP metadata: {baseURL}/api/auth/sso/saml2/sp/metadata?providerId=
                {p.providerId}
              </>
            )}
          </p>
        </div>
      ))}

      <form onSubmit={add} className="grid gap-3 border-t border-border pt-4 sm:grid-cols-2">
        <Field>
          Protocol
          <NativeSelect value={type} onChange={(e) => setType(e.target.value as "oidc" | "saml")}>
            <option value="oidc">OpenID Connect</option>
            <option value="saml">SAML 2.0</option>
          </NativeSelect>
        </Field>
        <Field>
          Provider id
          <Input name="providerId" placeholder="acme-okta" pattern="[a-z0-9-]{3,40}" required />
        </Field>
        <Field>
          Email domains
          <Input name="domain" placeholder="acme.com, acme.co.uk" required />
        </Field>
        <Field>
          {type === "oidc" ? "Issuer URL" : "IdP entity id"}
          <Input name="issuer" required />
        </Field>
        {type === "oidc" ? (
          <>
            <Field>
              Client id
              <Input name="clientId" required />
            </Field>
            <Field>
              Client secret
              <Input name="clientSecret" type="password" required />
            </Field>
            <Field className="sm:col-span-2">
              Discovery URL (optional)
              <Input name="discoveryEndpoint" placeholder="https://idp.example.com/.well-known/openid-configuration" />
            </Field>
          </>
        ) : (
          <>
            <Field className="sm:col-span-2">
              IdP sign-in URL
              <Input name="entryPoint" type="url" required />
            </Field>
            <Field className="sm:col-span-2">
              IdP signing certificate (PEM)
              <Textarea name="cert" required rows={4} className="font-mono text-xs" />
            </Field>
          </>
        )}
        <Button variant="brand" size="lg" type="submit" disabled={busy} className="justify-self-start">
          Add provider
        </Button>
      </form>
      <ErrorText>{error}</ErrorText>
    </Card>
  );
}
