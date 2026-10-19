import type { Metadata } from "next";
import { redirect } from "next/navigation";
import { connection } from "next/server";
import { AcceptInvitation } from "@/components/dashboard/AcceptInvitation";
import { getSession } from "@/lib/auth";

export const metadata: Metadata = { title: "Invitation" };

export default async function Page({ params }: PageProps<"/invitations/[id]">) {
  await connection();
  const { id } = await params;
  if (!(await getSession())) redirect(`/sign-in?next=${encodeURIComponent(`/invitations/${id}`)}`);
  return (
    <main className="flex flex-1 items-center justify-center p-6">
      <AcceptInvitation invitationId={id} />
    </main>
  );
}
