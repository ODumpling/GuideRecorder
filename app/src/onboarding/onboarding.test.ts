import { expect, it, vi } from "vitest";
import { allGranted, setupAtLaunch, setupAtStartup, setupStore } from "@/onboarding/onboarding";
import { AI_ENABLED } from "@/lib/features";
import type { LibraryInfo, Settings } from "@/lib/types";

const settings = (onboarded: boolean) => ({ general: { onboarded } }) as Settings;
const library = (error: string | null): LibraryInfo => ({
  path: "/lib",
  error,
  starting: false,
  fromEnv: false,
  defaultPath: "/lib",
});

it("shows setup on first launch or when the library is missing", () => {
  expect(setupAtLaunch(settings(false), library(null))).toBe("permissions");
  expect(setupAtLaunch(settings(true), library(null))).toBeNull();
  expect(setupAtLaunch(settings(true), library("the library folder /x is missing"))).toBe("library");
  expect(setupAtLaunch(settings(false), library("missing"))).toBe("library");
  // Not loaded yet: nothing to decide.
  expect(setupAtLaunch(null, null)).toBeNull();
});

it("needs every permission granted to record", () => {
  expect(allGranted(null)).toBe(false);
  expect(allGranted([])).toBe(false);
  expect(allGranted([{ permission: "accessibility", status: "granted" }])).toBe(true);
  expect(
    allGranted([
      { permission: "accessibility", status: "granted" },
      { permission: "input_monitoring", status: "not_determined" },
    ]),
  ).toBe(false);
});

it("opens and closes at a step", () => {
  setupStore.getState().open();
  expect(setupStore.getState().step).toBe("permissions");
  setupStore.getState().open("library");
  expect(setupStore.getState().step).toBe("library");
  setupStore.getState().close();
  expect(setupStore.getState().step).toBeNull();
});

it("waits for the library to open before deciding", async () => {
  const replies: LibraryInfo[] = [
    { ...library(null), starting: true },
    { ...library(null), starting: true },
    library("the library folder /Volumes/Team is missing"),
  ];
  let calls = 0;
  const step = await setupAtStartup(async () => replies[calls++]!, () => settings(true), { delayMs: 0 });
  expect(calls).toBe(3);
  expect(step).toBe("library");
  // A failing call doesn't block startup.
  expect(await setupAtStartup(() => Promise.reject(new Error("x")), () => settings(false), { delayMs: 0 })).toBe(
    "permissions",
  );
});

it("skips the permissions step on Windows, which asks for none", async () => {
  vi.stubGlobal("navigator", {
    userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0",
  });
  vi.resetModules();
  try {
    const onboarding = await import("@/onboarding/onboarding");
    expect(onboarding.STEPS).toEqual(AI_ENABLED ? ["library", "apiKey"] : ["library"]);
    expect(onboarding.setupAtLaunch(settings(false), library(null))).toBe("library");
    onboarding.setupStore.getState().open();
    expect(onboarding.setupStore.getState().step).toBe("library");
  } finally {
    vi.unstubAllGlobals();
    vi.resetModules();
  }
});
