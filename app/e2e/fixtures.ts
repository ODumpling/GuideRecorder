import { test as base, expect, type Page } from "@playwright/test";
import { Backend, type BackendOptions } from "./backend";

export { expect };

/** Fixture guide ids (fixtures/guides). */
export const CALENDAR = "01M4WT7MM000000000000002G0";
export const API_KEY = "01M4WZCE2000000000000002R0";

export const test = base.extend<{ backendOptions: BackendOptions; backend: Backend }>({
  backendOptions: [{}, { option: true }],
  backend: async ({ page, backendOptions }, use) => {
    const backend = new Backend(backendOptions);
    await page.exposeFunction("__guidesyaInvoke", (cmd: string, args: unknown) => backend.invoke(cmd, args));
    await page.goto("/e2e/index.html");
    await use(backend);
  },
});

/** Opens a guide from the library sidebar and waits for its outline. */
export async function openGuide(page: Page, title: string) {
  await page.getByRole("button", { name: new RegExp(title) }).click();
  await expect(page.getByRole("textbox", { name: "Guide title" })).toHaveValue(title);
}

/** The outline's item buttons, in order. */
export const outlineItems = (page: Page) => page.locator("[data-outline-item]");

/** Waits until the editor reports everything saved. */
export async function saved(page: Page) {
  await expect(page.getByText("Saved", { exact: true })).toBeVisible();
}

/** Emits an app event (e.g. `recording://status`) as the Rust side would. */
export async function emitEvent(page: Page, event: string, payload: unknown) {
  await page.evaluate(([e, p]) => window.__guidesyaEmit(e as string, p), [event, payload] as const);
}

/**
 * Loads the floating recording toolbar (toolbar.html) in place of the app,
 * against the same backend. Start a recording on `backend` first: the
 * toolbar reads the recorder's state once at load.
 */
export async function openToolbar(page: Page) {
  await page.goto("/e2e/index.html?page=toolbar");
  const toolbar = page.getByRole("toolbar", { name: "Recording toolbar" });
  await expect(toolbar).toBeVisible();
  return toolbar;
}

/**
 * Loads the region overlay (region.html) in place of the app, against the
 * same backend; `pick`: the pick id Rust puts in its URL.
 */
export async function openRegionOverlay(page: Page, pick?: number) {
  await page.goto(`/e2e/index.html?page=region${pick === undefined ? "" : `&pick=${pick}`}`);
  const overlay = page.getByLabel("Select a region to record");
  await expect(overlay).toBeFocused();
  return overlay;
}

/** Emits events in order (e.g. a held AI run's, from `backend.aiRelease()`). */
export async function emitEvents(page: Page, events: { event: string; payload: unknown }[]) {
  for (const { event, payload } of events) await emitEvent(page, event, payload);
}
