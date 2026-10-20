import { Link, useNavigate, useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import type { AuditSearch } from "@/lib/audit-filters";
import { saveAuditRetention, type getAuditLog } from "@/server/audit";
import { NativeSelect } from "@guiderecorder/ui/native-select";
import { Button } from "@guiderecorder/ui/button";
import { Card } from "@guiderecorder/ui/card";
import { Input } from "@guiderecorder/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@guiderecorder/ui/table";
import { ErrorText, Field } from "@/components/ui";

// The workspace's audit log: filters in the URL, the chain's check, CSV and
// JSON export of what's filtered, and how long events are kept.

type Data = Extract<Awaited<ReturnType<typeof getAuditLog>>, { allowed: true }>;

const CATEGORIES = ["user", "member", "invitation", "billing", "guide", "review", "policy", "share", "audit"];

const date = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "medium" });

export function AuditLog({ data, search }: { data: Data; search: AuditSearch }) {
  const navigate = useNavigate();
  const { workspaceId, events, chain, members } = data;
  const names = new Map(members.map((m) => [m.id, m.name]));

  function filter(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const value = (key: string) => String(form.get(key) ?? "").trim() || undefined;
    void navigate({
      to: "/audit",
      search: { action: value("action"), actorId: value("actorId"), targetId: value("targetId"), from: value("from"), to: value("to") },
    });
  }

  const { before: _, ...filters } = search;
  const exportUrl = (format: "csv" | "json") => {
    const params = new URLSearchParams({ format, ...Object.fromEntries(Object.entries(filters).filter(([, v]) => v !== undefined).map(([k, v]) => [k, String(v)])) });
    return `/api/workspaces/${workspaceId}/audit/export?${params}`;
  };

  return (
    <div className="mx-auto grid w-full max-w-6xl gap-7 px-5 py-8 md:px-9">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <h1 className="font-serif text-[40px] leading-none tracking-tight">Audit log</h1>
        <span className="flex gap-2">
          <Button asChild variant="outline">
            <a href={exportUrl("csv")}>Export CSV</a>
          </Button>
          <Button asChild variant="outline">
            <a href={exportUrl("json")}>Export JSON</a>
          </Button>
        </span>
      </header>

      <Card className="p-6 grid gap-3">
        {chain.ok ? (
          <p className="text-sm text-success">
            {chain.count} {chain.count === 1 ? "event" : "events"}, chain intact
            {chain.firstSeq && chain.firstSeq > 1 ? ` (older events pruned by the retention setting)` : ""}.
          </p>
        ) : (
          <p role="alert" className="text-sm text-destructive">
            The chain is broken at event {chain.seq}: {chain.reason} The log may have been tampered with.
          </p>
        )}
        <form onSubmit={filter} className="flex flex-wrap items-end gap-2 text-sm" key={JSON.stringify(filters)}>
          <Field>
            Action
            <NativeSelect name="action" defaultValue={search.action ?? ""}>
              <option value="">All</option>
              {CATEGORIES.map((c) => (
                <optgroup key={c} label={c}>
                  <option value={c}>All {c}</option>
                  {data.actions
                    .filter((a) => a.startsWith(`${c}.`))
                    .map((a) => (
                      <option key={a} value={a}>
                        {a}
                      </option>
                    ))}
                </optgroup>
              ))}
            </NativeSelect>
          </Field>
          <Field>
            Who
            <NativeSelect name="actorId" defaultValue={search.actorId ?? ""}>
              <option value="">Anyone</option>
              {members.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name} ({m.email})
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Field>
            Target id
            <Input name="targetId" defaultValue={search.targetId ?? ""} className="w-56" />
          </Field>
          <Field>
            From
            <Input name="from" type="date" defaultValue={search.from ?? ""} />
          </Field>
          <Field>
            To
            <Input name="to" type="date" defaultValue={search.to ?? ""} />
          </Field>
          <Button variant="brand" size="lg" type="submit">Filter</Button>
          {Object.values(filters).some(Boolean) ? (
            <Link to="/audit" className="h-9 content-center text-muted-foreground hover:underline">
              Clear
            </Link>
          ) : null}
        </form>
      </Card>

      <Card className="block p-0">
        <Table>
          <TableHeader className="[&_th]:text-muted-foreground">
            <TableRow>
              <TableHead className="px-4 py-2">#</TableHead>
              <TableHead className="px-4 py-2">When</TableHead>
              <TableHead className="px-4 py-2">Who</TableHead>
              <TableHead className="px-4 py-2">Action</TableHead>
              <TableHead className="px-4 py-2">Target</TableHead>
              <TableHead className="px-4 py-2">From</TableHead>
              <TableHead className="px-4 py-2">Details</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {events.map((e) => (
              <TableRow key={e.seq} className="align-top">
                <TableCell className="px-4 py-2 text-muted-foreground tabular-nums">{e.seq}</TableCell>
                <TableCell className="px-4 py-2 whitespace-nowrap">{date(e.createdAt)}</TableCell>
                <TableCell className="px-4 py-2">{e.actorId ? (e.actorName ?? names.get(e.actorId) ?? e.actorId) : <span className="text-muted-foreground">System</span>}</TableCell>
                <TableCell className="px-4 py-2 font-mono text-xs">{e.action}</TableCell>
                <TableCell className="px-4 py-2">
                  <span className="text-muted-foreground">{e.targetType}</span> <span className="font-mono text-xs break-all">{e.targetId}</span>
                </TableCell>
                <TableCell className="px-4 py-2 text-xs text-muted-foreground" title={e.userAgent ?? undefined}>
                  {e.ip ?? "—"}
                </TableCell>
                <TableCell className="px-4 py-2 font-mono text-xs break-all text-muted-foreground" title={`hash ${e.hash}`}>
                  {Object.keys(e.metadata as object).length ? JSON.stringify(e.metadata) : ""}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
        {!events.length ? <p className="p-4 text-sm text-muted-foreground">No events match.</p> : null}
      </Card>
      {data.hasMore ? (
        <Link to="/audit" search={{ ...filters, before: events.at(-1)!.seq }} className="justify-self-start text-sm text-brand hover:underline">
          Older events →
        </Link>
      ) : null}

      <Retention days={data.retentionDays} canSet={data.canSetRetention} />
    </div>
  );
}

function Retention({ days, canSet }: { days: number | null; canSet: boolean }) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const value = String(new FormData(event.currentTarget).get("days") ?? "").trim();
    setError(null);
    setSaved(false);
    const result = await saveAuditRetention({ data: { days: value ? Number(value) : null } });
    if (!result.ok) return setError(result.error);
    setSaved(true);
    await router.invalidate();
  }

  return (
    <Card className="p-6 grid gap-3">
      <h2 className="text-lg font-semibold">Retention</h2>
      <p className="text-sm text-muted-foreground">
        {days ? `Events older than ${days} days are deleted; the chain records where it now starts.` : "Events are kept forever."}
      </p>
      {canSet ? (
        <form onSubmit={save} className="flex flex-wrap items-end gap-2">
          <Field>
            Keep events for (days, empty for forever)
            <Input name="days" type="number" min={30} max={3650} defaultValue={days ?? ""} className="w-40" />
          </Field>
          <Button variant="brand" size="lg" type="submit">Save</Button>
          {saved ? <span className="text-sm text-muted-foreground">Saved.</span> : null}
        </form>
      ) : null}
      <ErrorText>{error}</ErrorText>
    </Card>
  );
}
