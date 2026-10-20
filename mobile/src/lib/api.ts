// Calls to the web app's API as the signed-in user. The session token comes
// from device sign-in (session.ts) and is kept in the Keychain.

import * as SecureStore from "expo-secure-store";
import { WEB_APP_URL } from "./config";

const TOKEN_KEY = "guiderecorder.session";

let token: string | null = null;

export async function loadToken(): Promise<string | null> {
  token = await SecureStore.getItemAsync(TOKEN_KEY);
  return token;
}

export async function saveToken(value: string | null): Promise<void> {
  token = value;
  if (value) await SecureStore.setItemAsync(TOKEN_KEY, value);
  else await SecureStore.deleteItemAsync(TOKEN_KEY);
}

export function authHeaders(): Record<string, string> {
  return token ? { authorization: `Bearer ${token}` } : {};
}

export interface ApiResponse<T = unknown> {
  status: number;
  body: T;
}

/** Calls `path` under `/api/`; the body is parsed JSON, or null. Rejects only when the request can't be made. */
export async function request<T>(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<ApiResponse<T>> {
  const response = await fetch(`${WEB_APP_URL}${path}`, {
    method,
    headers: {
      accept: "application/json",
      // Better Auth checks the origin of state-changing calls.
      origin: WEB_APP_URL,
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...authHeaders(),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let parsed: unknown = null;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = null;
  }
  return { status: response.status, body: parsed as T };
}

/** Why the server refused, from its JSON error body. */
export function refusal(response: ApiResponse): string {
  const body = response.body as { error?: string; message?: string } | null;
  if (body?.error) return body.error;
  if (body?.message) return body.message;
  if (response.status === 401) return "You're signed out. Sign in again.";
  if (response.status === 402) return "Syncing guides needs a Pro or Enterprise plan.";
  if (response.status === 403) return "You don't have permission to do that in this workspace.";
  return `The server answered ${response.status}.`;
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

/** `request`, throwing an `ApiError` unless the server answered 2xx. */
export async function call<T>(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<T> {
  const response = await request<T>(method, path, body);
  if (response.status < 200 || response.status >= 300) throw new ApiError(refusal(response), response.status);
  return response.body;
}
