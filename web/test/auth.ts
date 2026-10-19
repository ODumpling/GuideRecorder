import type { Database } from "@/db";
import { createAuth } from "@/lib/create-auth";
import type { Email } from "@/lib/email";

/** Better Auth over a test database, capturing sent emails. */
export function testAuth(db: Database) {
  const sent: Email[] = [];
  const auth = createAuth({
    db,
    baseURL: "http://localhost:3000",
    secret: "test-secret-test-secret-test-secret-0123",
    sendEmail: async (email) => {
      sent.push(email);
    },
  });

  /** Signs a new user up; returns their id and session cookie headers. */
  async function signUp(name: string, email = `${name.toLowerCase()}@example.com`) {
    const response = await auth.api.signUpEmail({
      body: { name, email, password: "correct horse battery" },
      asResponse: true,
    });
    const body = (await response.json()) as { user: { id: string } };
    const cookie = response.headers.getSetCookie().map((c) => c.split(";")[0]).join("; ");
    return { userId: body.user.id, headers: new Headers({ cookie }) };
  }

  return { auth, sent, signUp };
}
