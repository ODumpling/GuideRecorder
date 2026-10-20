import { useState } from "react";
import { addScimConnection, endScimConnection, revokeScim, rotateScim } from "@/server/admin";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { ErrorText } from "@/components/ui";
import { useAction } from "./useAction";

interface Connection {
  connectionId: string;
  status: string;
  createdAt: Date;
  credentials: { credentialId: string; expiresAt: Date; lastUsedAt: Date | null }[];
}

const day = (d: Date | null) => (d ? new Date(d).toLocaleDateString() : "never");

/** SCIM connections and their tokens. A token is shown once, right after it's made. */
export function ScimConnections({ connections, endpoint }: { connections: Connection[]; endpoint: string }) {
  const { run, error, busy } = useAction();
  const [token, setToken] = useState<string | null>(null);

  return (
    <Card className="p-6 grid gap-4">
      <div>
        <h2 className="text-lg font-semibold">SCIM provisioning</h2>
        <p className="text-sm text-muted-foreground">
          Your directory adds, updates and removes members. Groups named Admins, Approvers, Editors or Viewers set roles. SCIM base URL:{" "}
          <code className="font-mono text-xs">{endpoint}</code>
        </p>
      </div>

      {token && (
        <div className="grid gap-1 rounded-md bg-muted p-3 text-sm">
          <span className="font-medium">Copy this token now; it won&rsquo;t be shown again.</span>
          <code className="break-all font-mono text-xs">{token}</code>
        </div>
      )}

      {connections.map((c) => (
        <div key={c.connectionId} className="grid gap-2 rounded-lg border border-border p-4 text-sm">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>
              <span className="font-mono text-xs">{c.connectionId}</span> <span className="text-muted-foreground">{c.status}</span>
            </span>
            <span className="flex gap-2">
              <Button
                variant="outline"
                disabled={busy}
                onClick={async () => {
                  const result = await run(() => rotateScim({ data: { connectionId: c.connectionId } }));
                  if (result) setToken(result.token);
                }}
              >
                New token
              </Button>
              <Button variant="destructive" disabled={busy} onClick={() => run(() => endScimConnection({ data: { connectionId: c.connectionId } }))}>
                Disconnect
              </Button>
            </span>
          </div>
          <ul className="grid gap-1 text-xs text-muted-foreground">
            {c.credentials.map((k) => (
              <li key={k.credentialId} className="flex items-center justify-between gap-2">
                <span>
                  Token expires {day(k.expiresAt)}, last used {day(k.lastUsedAt)}
                </span>
                <Button
                  variant="outline" size="lg"
                  className="h-7 px-2"
                  disabled={busy}
                  onClick={() => run(() => revokeScim({ data: { connectionId: c.connectionId, credentialId: k.credentialId } }))}
                >
                  Revoke
                </Button>
              </li>
            ))}
          </ul>
        </div>
      ))}

      <Button variant="brand" size="lg"
        className="justify-self-start"
        disabled={busy}
        onClick={async () => {
          const result = await run(() => addScimConnection());
          if (result) setToken(result.token);
        }}
      >
        New connection
      </Button>
      <ErrorText>{error}</ErrorText>
    </Card>
  );
}
