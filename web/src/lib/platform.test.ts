import { beforeEach, describe, expect, it } from "vitest";
import { testAuth } from "../../test/auth";
import { testDb } from "../../test/db";
import type { Database } from "@/db";
import { subscription } from "@/db/schema";
import { DEFAULT_PLATFORM, getPlatformSettings, offeredPlans, setPlatformSettings } from "./platform";

let db: Database;
let t: ReturnType<typeof testAuth>;

beforeEach(async () => {
  db = await testDb();
  t = testAuth(db);
});

const signUp = (email: string) =>
  t.auth.api.signUpEmail({ body: { name: "New", email, password: "correct horse battery" }, asResponse: true });

describe("settings", () => {
  it("default to open registration and every plan", async () => {
    expect(await getPlatformSettings(db)).toEqual(DEFAULT_PLATFORM);
  });

  it("save, keeping plans in order and rejecting unknown ones", async () => {
    await setPlatformSettings(db, "staff", { disabledPlans: ["enterprise", "pro"] });
    expect(await getPlatformSettings(db)).toEqual({ registration: true, disabledPlans: ["pro", "enterprise"] });
    await setPlatformSettings(db, "staff", { registration: false });
    expect(await getPlatformSettings(db)).toEqual({ registration: false, disabledPlans: ["pro", "enterprise"] });
    await expect(setPlatformSettings(db, "staff", { disabledPlans: ["free" as never] })).rejects.toThrow();
  });
});

describe("offeredPlans", () => {
  it("are the priced plans that aren't disabled", () => {
    expect(offeredPlans(DEFAULT_PLATFORM, { pro: "price_pro", enterprise: "price_ent" })).toEqual(["pro", "enterprise"]);
    expect(offeredPlans(DEFAULT_PLATFORM, { pro: "price_pro" })).toEqual(["pro"]);
    expect(offeredPlans({ registration: true, disabledPlans: ["pro"] }, { pro: "price_pro", enterprise: "price_ent" })).toEqual(["enterprise"]);
  });
});

describe("registration", () => {
  it("is open by default", async () => {
    expect((await signUp("ada@example.com")).status).toBe(200);
  });

  it("refuses new accounts when closed", async () => {
    await setPlatformSettings(db, "staff", { registration: false });
    const response = await signUp("ada@example.com");
    expect(response.status).toBe(403);
    expect(await response.json()).toMatchObject({ code: "REGISTRATION_CLOSED" });
  });

  it("still lets invited people join", async () => {
    const owner = await t.signUp("Ada");
    const organizationId = (await t.auth.api.getSession({ headers: owner.headers }))!.session.activeOrganizationId!;
    await db.insert(subscription).values({ id: "sub", plan: "enterprise", referenceId: organizationId, status: "active", seats: 5 });
    await setPlatformSettings(db, "staff", { registration: false });
    await t.auth.api.createInvitation({ headers: owner.headers, body: { organizationId, email: "grace@example.com", role: "editor" } });

    expect((await signUp("grace@example.com")).status).toBe(200);
    expect((await signUp("eve@example.com")).status).toBe(403);
  });

  it("doesn't stop existing users signing in", async () => {
    await t.signUp("Ada");
    await setPlatformSettings(db, "staff", { registration: false });
    const response = await t.auth.api.signInEmail({ body: { email: "ada@example.com", password: "correct horse battery" }, asResponse: true });
    expect(response.status).toBe(200);
  });
});
