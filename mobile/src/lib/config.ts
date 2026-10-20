/** The web app to sign in to, from mobile/.env (mobile/.env.development under `expo start`). */
export const WEB_APP_URL = (process.env.EXPO_PUBLIC_WEB_APP_URL ?? "https://gr.dufflebear.dev").replace(/\/+$/, "");

/** This app's client id for device authorization (web/src/lib/create-auth.ts). */
export const CLIENT_ID = "guiderecorder-mobile";
