/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** The web app to sign in to and sync with (app/.env). */
  readonly VITE_WEB_APP_URL: string;
}
