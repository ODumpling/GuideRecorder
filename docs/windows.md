# GuideRecorder on Windows

The Windows app has every feature of the macOS app: recording a screen, a
window or a region, typed text, scrolling and web addresses in steps,
narration, re-capture and inserting recorded steps, the editor, AI writing,
redaction, version history, every export (MP4 included), agent skills with a
replay script, SCORM, annotation guides with PDF import, sync, the MCP
server and the command line.

Where a feature works differently on Windows, this page says how and why.
Anything not listed here works as the README describes.

## Requirements

- Windows 11, or Windows 10 version 2004 or later, 64-bit. Recording uses
  Windows.Graphics.Capture; Windows 10 may draw its yellow capture border
  around what is recorded while recording runs, as only Windows 11 lets an
  app capture without it. (Tested on Windows 11.)
- The WebView2 runtime. The installer downloads it if it is missing (it
  comes with Windows 11).
- No permissions to grant for recording. Desktop apps may capture the screen,
  watch the mouse and keyboard and read UI Automation, so there is no
  Permissions page and no permissions step in onboarding.
- To narrate: the microphone allowed for desktop apps, and a desktop speech
  recogniser for your language (see [Narration](#narration)).
- For MP4 export and narration audio: Media Foundation. It is part of
  Windows except the N editions, which need the free Media Feature Pack. The
  app starts without it; only those two features say what to install.

## Installing and updating

- The installer is a per-user NSIS installer (no administrator rights),
  into `%LOCALAPPDATA%\GuideRecorder`. It is **not code-signed yet**, so
  SmartScreen asks before the first run ("More info" → "Run anyway").
- Updates are found and installed the same way as on macOS. **Check for
  Updates…** is in the tray icon's menu, since Windows apps have no app menu.

## Where things are kept

| | macOS | Windows |
| --- | --- | --- |
| Library (default) | `~/Documents/GuideRecorder` | `Documents\GuideRecorder` |
| Settings, sync state, search index | `~/Library/Application Support/dev.dufflebear.guiderecorder` | `%LOCALAPPDATA%\dev.dufflebear.guiderecorder` |
| Recordings in progress | `~/Library/Caches/dev.dufflebear.guiderecorder/sessions` | `%LOCALAPPDATA%\dev.dufflebear.guiderecorder\sessions` |
| API keys, sync token | Keychain | Windows Credential Manager |

The app's folder is local rather than roaming: it names folders on this PC
(the library) and holds a database, neither of which should follow you to
another PC. The CLI's `--app-index` and the MCP server's default `--index`
use the same index.

## The app's window, tray and shortcuts

- **No menu bar.** Everything is in the app's own window, the command
  palette (Ctrl+K) and the tray icon. On a Mac the app menu has Check for
  Updates… and Quit; on Windows the tray menu has both.
- **Tray icon** (the notification area; Windows may put it in the overflow
  next to the clock until you drag it out). A click opens GuideRecorder and
  a right-click shows the menu: start, pause, resume and stop a recording,
  open the app, check for updates, quit. The recorded time shows in the
  icon's tooltip, as Windows shows no text beside a tray icon. The icon is
  white on a dark taskbar and black on a light one.
- **Shortcuts** are written the Windows way (Ctrl+Shift+Z rather than ⇧⌘Z).
  The recording defaults are **Ctrl+Alt+R** (record), **Ctrl+Alt+P**
  (pause) and **Ctrl+Alt+S** (manual step). Shortcuts recorded in a guide
  read "Ctrl+S", "Enter" and so on.
- **Logging off or shutting down** while GuideRecorder runs closes it
  without the save-on-quit step that macOS gets for a logout. The editor
  saves as you type, so at most the last moment of an edit is lost, but a
  recording still running is not saved; stop it first.

## Recording

- **Apps are identified by their executable** (`1password.exe`), compared
  without regard to case, where macOS uses bundle ids. Settings → Privacy
  takes executables, and the suggested exclusions are 1Password, Bitwarden,
  KeePass, KeePassXC, Enpass and the Windows credential prompt.
- **Coordinates are physical pixels.** Windows has no single logical space
  across displays with different scaling, so click points, regions and
  frames are in physical pixels and every display has a scale of 1. This
  is invisible in the app; it matters only to someone reading
  `guide.json` or `steps.json`.
- **Element roles** come from UI Automation control types, mapped to the
  macOS names (an Edit is stored as `AXTextField`), so guides read the same
  on both. A password field is recognised by UI Automation's `IsPassword`,
  and a lookup that fails counts as a password field (keys are masked).
- **Scrolling** is measured in points on macOS; Windows reports wheel
  notches, so the distance is estimated from them.
- **Web addresses** are read from the browser's address bar (its UI
  Automation value) rather than from the page, in Chrome, Edge, Firefox,
  Brave and Arc. Private and incognito windows record none, as on macOS.

### Excluded apps' windows in screenshots

On macOS, ScreenCaptureKit leaves an excluded app's windows out of every
screenshot, even behind other apps, and shows what is underneath.
Windows.Graphics.Capture cannot leave windows out, so GuideRecorder **paints
over** them instead: in every screenshot of a screen or region, the visible
part of each window of an excluded app (with its menus, tooltips and
popups) is filled with flat grey. As on macOS, nothing is captured while an
excluded app is in front. The differences:

- The area is grey, not the windows behind it. The grey reaches 4 px past
  the window, and an 8 px band of the edges of windows in front of it is
  grey too, to cover rounded corners and small moves.
- The windows are found a few milliseconds after Windows composed the
  frame, so a window that moves, opens or appears in that moment can show
  at its edges for a frame.
- Apps that draw their own transparency (a see-through terminal, acrylic
  effects) count as opaque, so what shows through them is not painted over.
  Treating them as see-through would grey out most browser windows whenever
  a password manager is behind them.
- Windows that other processes draw for an excluded app are not covered:
  input method candidate lists, shell flyouts, notifications.
- When recording a single window, other windows never appear in the
  capture, so there is nothing to paint over (as on macOS).

GuideRecorder's own windows (the toolbar, the click ring) are kept out of
screenshots by display affinity, as ScreenCaptureKit does on macOS.

## Narration

Narration works as on macOS: the toolbar's microphone, off by default,
transcribed on the device while you record, split across the steps by
time, with the audio kept as `narration.m4a` if you ask. What differs is
the speech engine.

- **Engine.** Windows' desktop speech recogniser (SAPI 5 dictation),
  running inside GuideRecorder, rather than Apple's Speech framework. Like
  on macOS, nothing is sent anywhere. Windows' newer speech API is not used
  because its dictation sends audio to Microsoft's online service.
- **Languages.** It needs a desktop recogniser, which comes with a
  language's speech pack in Settings → Time & language → Speech. Only some
  languages have one, typically English (US, UK), German, French, Spanish,
  Japanese and Chinese. GuideRecorder picks the one for your Windows display
  language, then a variety of the same language, then the default one. So a
  user with only an English recogniser gets English. With none installed,
  narration says to add one rather than start.
- **Punctuation** is not added for you. Spoken punctuation ("comma", "full
  stop") is written. Each step's description still ends with a full stop.
- **Accuracy** is lower than Apple's on-device model, especially before
  Windows has learned your voice. It improves as the recogniser adapts to
  your voice over time.
- **Permissions.** Windows has no prompt for desktop apps. Settings →
  Recording shows whether the microphone is allowed (Settings → Privacy &
  security → Microphone, including "Let desktop apps access your
  microphone") and whether a speech recogniser is installed, with buttons
  that open those pages. Onboarding does not ask, as narration is optional.
- **Audio** is kept as mono AAC at about 96 kbit/s (stereo at 64 kbit/s a
  channel on macOS). On a Windows N edition without the Media Feature Pack,
  narration still works but no audio is kept.

## Exports

- **MP4** is encoded with Media Foundation (H.264, the same size and bit
  rate as on macOS), with hardware encoding when there is one.
- **PDF import** for annotation guides draws pages with Windows' own PDF
  renderer (Windows.Data.Pdf), at the same size, crop box and rotation as on
  macOS. A password-protected PDF is refused.
- **Text recognition** for redaction uses Windows OCR with your profile's
  languages (else English), where macOS uses Vision. The two engines can find
  slightly different text, so review the suggested blurs as on macOS.
- **Agent skills** of a guide recorded on Windows come with **`replay.ps1`**
  instead of `replay.js` (see below). A guide recorded on a Mac gets
  `replay.js` on either platform, and each script refuses the other's
  guides.

### replay.ps1

A Windows PowerShell script (5.1, built into Windows) that repeats the steps
through UI Automation, as `replay.js` does through the macOS accessibility
API, with the same safety stops (an unclear, missing or ambiguous element, a
password, a manual step, a double-click or a drag).

```powershell
powershell -ExecutionPolicy Bypass -File replay.ps1 -DryRun
powershell -ExecutionPolicy Bypass -File replay.ps1 -From 4 -Delay 2
```

How it differs from `replay.js`:

- PowerShell options: `-DryRun`, `-From N`, `-To N`, `-Delay S`, `-Help`. It
  exits with code 1 when it stops at a step.
- The app is found by its executable, the window by its title, the element
  by its control type (each recorded role stands for the control types it
  came from) and its name, help text, label or value. A label cut at 200
  characters when recorded must match exactly as cut, rather than as a
  prefix.
- A click uses whichever of Invoke, Toggle, Select or Expand the element
  supports. A right-click focuses the element and presses Shift+F10, since UI
  Automation has no "show menu" action.
- Keys are sent with `SendInput`: text as Unicode characters (any keyboard
  layout, emoji included), and shortcuts with Ctrl, Alt, Shift and the
  Windows key. A punctuation key that needs AltGr on your layout stops as
  unknown.
- A shortcut goes to the app of its step (or of the step before), not to
  whatever is in front; a step that names no app stops rather than guess.
- It can't control an app running as administrator unless it runs as
  administrator too (Windows' rule for all input automation).

## The command line and the MCP server

Both run on Windows (`guiderecorder.exe`, `guiderecorder-mcp.exe`, installed
next to the app). Settings → MCP gives the `claude mcp add` command with the
paths quoted for PowerShell. The MCP server's `get_skill` with `replay`
returns `replay.ps1` for a guide recorded on Windows.

## Building from source

As in the README: Rust, Node.js and pnpm, then `pnpm tauri dev` in `app/`.
You also need the Visual Studio C++ build tools (the "Desktop development
with C++" workload) and the Windows SDK, which Rust's MSVC toolchain uses.
There is no signing identity to set up: Windows ties no permissions to the
app's signature. `docs/releasing.md` says how to build the installer.
