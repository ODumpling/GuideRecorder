import type { Metadata } from "next";
import { redirect } from "next/navigation";
import { connection } from "next/server";
import { AuthForm } from "@/components/AuthForm";
import { authFeatures, getSession } from "@/lib/auth";
import { safeNext } from "@/lib/redirect";

export const metadata: Metadata = { title: "Sign up" };

export default async function Page({ searchParams }: PageProps<"/sign-up">) {
  await connection();
  const next = safeNext((await searchParams).next);
  if (await getSession()) redirect(next);
  const { google, microsoft } = authFeatures();
  return (
    <main className="flex flex-1 items-center justify-center p-6">
      <AuthForm mode="sign-up" providers={{ google, microsoft }} next={next} />
    </main>
  );
}
