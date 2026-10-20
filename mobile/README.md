# GuideRecorder for iPhone and iPad

Read your workspace's guides and add steps to them with the camera. Expo
(SDK 57, the version the App Store's Expo Go runs), React Native and Expo Router; the guide types and the three-way
merge come from `packages/guide-schema` and `packages/sync`, shared with the
desktop and web apps, and the colour themes match `packages/ui/src/theme.css`.

- **Sign in** as the desktop app does: the app gets a device code from the
  web app and opens `/device` in a browser sheet that shares Safari's
  sign-in; once approved, it stores the session token in the Keychain and
  calls the API with it as a bearer token (client id `guiderecorder-mobile`).
- **Local-first, like the desktop app.** Guides live on the phone
  (`Documents/guides/<id>/guide.json` and their images, laid out like the
  desktop app's bundles, `src/lib/device.ts`) and work offline.
- **Library**: two tabs. *On this phone* lists the guides here with their
  sync state; *Web* lists every guide in the chosen workspace (followed
  through the change feed and cached), marking the ones already on the
  phone. Search filters by title, tags and summary. On iPad (and any window
  768 points or wider) the selected guide shows beside the list.
- **Sync** (`src/lib/sync.ts`) runs the desktop app's sync loop
  (`@guiderecorder/sync/engine`, moved out of the desktop app so both use
  it) over a backend for the phone: it pulls the feed, pushes changed
  guides with their images, merges when both sides changed a guide, and
  flags clashes. You choose which guides go up: each guide has a **Sync
  with web** switch (on by default for new guides when signed in). Any web
  guide can be **downloaded**; a downloaded guide stays synced. Clashing
  edits ask which to keep: this phone's, the web's, or both (a copy). Sync
  runs at start, a moment after each edit, every minute while the app is
  open, when it comes back to the front, and on pull to refresh.
  **Remove from this phone** deletes the local copy only.
- **Guides** read like the web viewer: numbered steps with their images,
  sections, notes, intro and prerequisites. Annotations aren't drawn yet.
- **Take photo**: take a photo with the back or front camera (flash off,
  auto or on), give it a title and description, and add it; then take the
  next. Photos are scaled to 2560 pixels on the long side and saved as JPEG
  at `steps/<step id>.jpg` in the guide on the phone; sync uploads them if
  the guide syncs. New steps are `manual` steps showing the full image; an
  untitled one is left for **Write with AI** in the desktop app.
- **From a screen recording**: record the task in any app with iOS's
  Screen Recording, then choose the video (trimming it if you like). The
  app samples it twice a second as tiny frames, and each screen that holds
  still for a moment and differs from the previous step becomes a step,
  captured just before it changes (`src/lib/frames.ts`); the status bar and
  home indicator are ignored, so the clock doesn't count as a change. You
  review the screens, remove the ones you don't need, title the rest, and
  add them. iOS never tells an app where you tapped in other
  apps, so steps have no click point and untitled ones are left for
  **Write with AI**.
- **New guide**: a title, then straight to the camera or a recording.
- **Settings**: account, workspace, sync status and **Sync now**, theme (the desktop app's six, or match
  the system: Arctic in light mode, Paper Dark in dark mode), sign out.

Syncing needs a Pro or Enterprise workspace, as for the desktop app. Images
are shown through `GET /api/workspaces/:id/assets/:sha256` with
`Accept: application/json`, which answers the signed URL instead of
redirecting to it, so the bearer token never goes to the object store.

## Run it

```sh
pnpm install              # from the repository root
cd mobile
pnpm start                # then press i for the iOS simulator, or scan the QR code
```

The camera needs a real device. Expo Go runs the app as is (every module it
uses ships with Expo Go); for your own build, `npx expo run:ios` (Xcode) or
`npx eas-cli@latest build --profile development --platform ios` (no Xcode).

`pnpm start` signs in to `https://app.guiderecorder.dev:3000`
(`.env.development`), release builds to `https://app.guiderecorder.app`
(`.env`). A phone can't reach the dev domain unless it trusts the local CA
and resolves the name; otherwise set `EXPO_PUBLIC_WEB_APP_URL` in
`.env.local` to an address it can reach (and set the web app's
`BETTER_AUTH_URL` to match).

```sh
pnpm typecheck
pnpm test                 # vitest: guide building, feed, merge inputs, Markdown, step detection
```

## Layout

```
src/app/          routes: library (index), guides/[id], guides/[id]/capture, new-guide, settings, sign-in
src/components/   UI building blocks (after packages/ui), guide view, images, Markdown
src/lib/          API and sign-in, library store, push and merge, photos and uploads, pure guide helpers
src/theme/        colour themes and fonts (Inter, Instrument Serif)
```
