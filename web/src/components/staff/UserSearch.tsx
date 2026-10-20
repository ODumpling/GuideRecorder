import { Link, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { Button } from "@guiderecorder/ui/button";
import { Input } from "@guiderecorder/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@guiderecorder/ui/table";
import type { searchUsers } from "@/server/staff";
import { day } from "./format";

type Users = Awaited<ReturnType<typeof searchUsers>>;

/** Finds users by name, email or id; the newest without a query. */
export function UserSearch({ users, query }: { users: Users; query: string }) {
  const navigate = useNavigate();
  const [q, setQ] = useState(query);

  function search(event: FormEvent) {
    event.preventDefault();
    void navigate({ to: "/staff", search: q.trim() ? { q: q.trim() } : {} });
  }

  return (
    <>
      <div>
        <h1 className="font-serif text-[40px] leading-none tracking-tight">Users</h1>
        <p className="mt-2 text-sm text-muted-foreground">{query ? `Matching “${query}”` : "Newest first"} · up to 50</p>
      </div>
      <form onSubmit={search} className="flex gap-2">
        <Input autoFocus placeholder="Name, email or user id" value={q} onChange={(e) => setQ(e.target.value)} className="max-w-md" />
        <Button type="submit">Search</Button>
      </form>
      {users.length ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>User</TableHead>
              <TableHead>Workspaces</TableHead>
              <TableHead>Signed up</TableHead>
              <TableHead>Last seen</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {users.map((u) => (
              <TableRow key={u.id}>
                <TableCell>
                  <Link to="/staff/users/$userId" params={{ userId: u.id }} className="font-medium hover:underline">
                    {u.name || "(no name)"}
                  </Link>{" "}
                  <span className="text-muted-foreground">{u.email}</span>
                  {u.emailVerified ? null : <span className="ml-2 text-xs text-status-warning">unverified</span>}
                </TableCell>
                <TableCell>{u.workspaces}</TableCell>
                <TableCell>{day(u.createdAt)}</TableCell>
                <TableCell>{u.lastSeenAt ? day(u.lastSeenAt) : "Never"}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : (
        <p className="rounded-xl border border-dashed border-border px-6 py-14 text-center text-sm text-muted-foreground">No users found.</p>
      )}
    </>
  );
}
