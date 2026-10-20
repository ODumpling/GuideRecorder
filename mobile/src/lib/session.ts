// Signing in, as the desktop app does: the web app issues a device code, the
// user approves it on /device (in an in-app browser sheet that shares
// Safari's sign-in), and the app polls for the session token.

import AsyncStorage from "@react-native-async-storage/async-storage";
import * as Linking from "expo-linking";
import * as WebBrowser from "expo-web-browser";
import { create } from "zustand";
import { call, loadToken, request, saveToken } from "./api";
import { CLIENT_ID, WEB_APP_URL } from "./config";

export interface User {
  id: string;
  name: string;
  email: string;
}

export interface Workspace {
  id: string;
  name: string;
}

export interface PendingSignIn {
  userCode: string;
  url: string;
}

interface SessionStore {
  status: "loading" | "signedOut" | "signedIn";
  user: User | null;
  workspaces: Workspace[];
  workspaceId: string | null;
  signIn: PendingSignIn | null;
  error: string | null;
  load(): Promise<void>;
  startSignIn(): Promise<void>;
  openSignInPage(): Promise<void>;
  cancelSignIn(): void;
  signOut(): Promise<void>;
  chooseWorkspace(id: string): void;
  loadWorkspaces(): Promise<void>;
}

const WORKSPACE_KEY = "guiderecorder.workspace";

/** Bumped to stop a sign-in poll that was cancelled or replaced. */
let attempt = 0;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The page as an absolute URL on the web app: never open a page the response picked elsewhere. */
function onServer(uri: string): string | null {
  try {
    const url = new URL(uri, WEB_APP_URL);
    return url.origin === new URL(WEB_APP_URL).origin ? url.toString() : null;
  } catch {
    return null;
  }
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const sessionStore = create<SessionStore>((set, get) => ({
  status: "loading",
  user: null,
  workspaces: [],
  workspaceId: null,
  signIn: null,
  error: null,

  async load() {
    if (!(await loadToken())) return set({ status: "signedOut" });
    try {
      const session = await request<{ user: User } | null>("GET", "/api/auth/get-session");
      if (session.status === 401 || (session.status === 200 && !session.body?.user)) {
        await saveToken(null);
        return set({ status: "signedOut" });
      }
      if (session.status !== 200) throw new Error(`The server answered ${session.status}.`);
      set({ user: session.body!.user, workspaceId: await AsyncStorage.getItem(WORKSPACE_KEY) });
      await get().loadWorkspaces();
      set({ status: "signedIn", error: null });
    } catch (error) {
      // Offline: keep the sign-in and show what's cached.
      set({ status: "signedIn", error: message(error), workspaceId: await AsyncStorage.getItem(WORKSPACE_KEY) });
    }
  },

  async loadWorkspaces() {
    const workspaces = (await call<Workspace[]>("GET", "/api/auth/organization/list")).map(({ id, name }) => ({ id, name }));
    set({ workspaces });
    const current = get().workspaceId;
    if (!current || !workspaces.some((w) => w.id === current)) get().chooseWorkspace(workspaces[0]?.id ?? "");
  },

  async startSignIn() {
    const mine = ++attempt;
    set({ error: null, signIn: null });
    let code: {
      device_code: string;
      user_code: string;
      verification_uri_complete: string;
      expires_in: number;
      interval: number;
    };
    try {
      const response = await request<typeof code>("POST", "/api/auth/device/code", { client_id: CLIENT_ID });
      if (response.status !== 200) throw new Error(`${WEB_APP_URL} doesn't look like a GuideRecorder web app (${response.status}).`);
      code = response.body;
    } catch (error) {
      return set({ error: message(error) });
    }
    const url = onServer(code.verification_uri_complete);
    if (!url) return set({ error: "The server sent an unexpected sign-in page." });
    set({ signIn: { userCode: code.user_code, url } });
    void get().openSignInPage();

    let interval = Math.max(code.interval, 1) * 1000;
    const deadline = Date.now() + code.expires_in * 1000;
    while (mine === attempt && Date.now() < deadline) {
      await sleep(interval);
      if (mine !== attempt) return;
      const poll = await request<{ access_token?: string; error?: string; error_description?: string }>("POST", "/api/auth/device/token", {
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        device_code: code.device_code,
        client_id: CLIENT_ID,
      }).catch(() => null);
      if (!poll) continue;
      const body = poll.body ?? {};
      if (body.access_token) {
        await saveToken(body.access_token);
        WebBrowser.dismissBrowser().catch(() => {});
        set({ signIn: null });
        await get().load();
        return;
      }
      if (body.error === "authorization_pending") continue;
      if (body.error === "slow_down") {
        interval += 5000;
        continue;
      }
      WebBrowser.dismissBrowser().catch(() => {});
      const why =
        body.error === "access_denied"
          ? "Sign-in was denied."
          : body.error === "expired_token"
            ? "The code expired. Try again."
            : (body.error_description ?? "Sign-in failed.");
      return set({ signIn: null, error: why });
    }
    if (mine === attempt) set({ signIn: null, error: "The code expired. Try again." });
  },

  async openSignInPage() {
    const url = get().signIn?.url;
    // Approval is noticed by polling, which closes the sheet; nothing redirects back.
    if (url) await WebBrowser.openAuthSessionAsync(url, Linking.createURL("signed-in")).catch(() => {});
  },

  cancelSignIn() {
    attempt++;
    set({ signIn: null });
  },

  async signOut() {
    await request("POST", "/api/auth/sign-out", {}).catch(() => {});
    await saveToken(null);
    set({ status: "signedOut", user: null, workspaces: [], signIn: null, error: null });
  },

  chooseWorkspace(id) {
    set({ workspaceId: id || null });
    if (id) void AsyncStorage.setItem(WORKSPACE_KEY, id);
    else void AsyncStorage.removeItem(WORKSPACE_KEY);
  },
}));
