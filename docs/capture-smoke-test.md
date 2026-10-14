# Capture smoke test (manual)

The capture layer talks to macOS (CGEventTap, ScreenCaptureKit, the
Accessibility API) and needs TCC permissions, so it is not covered by CI.
Tests that need permissions are `#[ignore]`d. Run this checklist by hand
after changing anything in `crates/guidesya-capture`, after a macOS update,
and before a release.

Each example runs for a few seconds, prints what it sees, and exits.

## Before you start

- [ ] Note the macOS version and the Mac model (Retina or not, number of
      displays). Write them in the results.
- [ ] Build once: `cargo build -p guidesya-capture --examples`.
- [ ] Grant permissions to **the terminal app you run the examples from**
      (Terminal, iTerm, Ghostty, …), not to the binary. Open the panes in
      System Settings → Privacy & Security:

  | Permission       | Needed by                  | Settings link                                                              |
  | ---------------- | -------------------------- | -------------------------------------------------------------------------- |
  | Input Monitoring | `event_tap` (keyboard), `record` | `x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent` |
  | Accessibility    | `ax`, `record`, `event_tap` (some versions) | `x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility` |
  | Screen Recording | `frames`, `record`         | `x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture` |

- [ ] Quit and reopen the terminal after granting. Most grants only take
      effect for newly started processes.
- [ ] Optional: start with the permissions **off** for one run of each
      example, to check the failure paths described below.

## 1. Event tap: `cargo run -p guidesya-capture --example event_tap`

Listens for 10 s and prints one JSON line per event. `targetPid` and `ts`
are at the top level, and the event itself is under `event`, tagged by
`type`. Key fields such as `characters` sit under `event.key`. For example
(wrapped here; each event is one line):

```text
{"targetPid":412,"ts":183920451230000,"event":{"type":"mouseDown",
 "point":{"x":812.5,"y":440.0},"button":"left","clickCount":1,
 "modifiers":{"shift":false,"control":false,"option":false,"command":false,"function":false,"capsLock":false}}}
{"targetPid":412,"ts":183921903114000,"event":{"type":"keyDown",
 "key":{"code":1,"characters":"s","charactersIgnoringModifiers":"s"},
 "modifiers":{"shift":false,"control":false,"option":false,"command":false,"function":false,"capsLock":false},
 "isRepeat":false,"secure":false}}
```

- [ ] Without Input Monitoring (or Accessibility), either outcome is
      acceptable:
  - It prints a `warning: … is …` line naming the missing permission and
    its settings link, then keeps listening. Mouse events may still arrive,
    but key events do not.
  - macOS refuses the tap. It prints `error: CGEventTapCreate failed (check
    Input Monitoring / Accessibility permission)` and exits with status 1.

  Either way it must not crash or hang.
- [ ] With permissions, click in **another** app's window (Finder, Safari).
      You should see `mouseDown` / `mouseUp` with the click position, the
      right `button`, and `clickCount` 2 for a double-click.
- [ ] `targetPid` is the pid of the app you clicked (check it in Activity
      Monitor), **not 0**. Events aimed at the example's own process (the pid
      it prints at start) are skipped.
- [ ] Right-click reports the right button. Middle or side buttons report
      `other`. Note which physical button you pressed: the contract can't
      tell them apart yet (see todo #336).
- [ ] A drag (press, move, release) gives `mouseDown`, several
      `mouseDragged`, then `mouseUp`.
- [ ] Scrolling gives `scroll` events.
- [ ] Typing gives `keyDown` with `event.key.characters` and
      `event.key.charactersIgnoringModifiers`. Try a non-US layout if you have one.
- [ ] ⌘, ⌥, ⌃ and ⇧ give `flagsChanged`, and shortcuts such as ⌘C give
      `keyDown` with the modifiers set. Left and right modifiers can't be
      told apart yet (no keycode on `flagsChanged`; see #336).
- [ ] `MouseDown.element` and `KeyDown.secure` are unset here. The capture
      session fills them in, not the tap.
- [ ] **Timestamps, sanity check:** two events about 1 s apart (count
      "one-Mississippi") should have `ts` values about 1 000 000 000 apart.
      `ts` is already converted to host nanoseconds by the tap, so this
      only shows that the conversion works.
- [ ] After 10 s it prints `stopped` and exits by itself.

### 1b. Hardware timestamp unit: `cargo run -p guidesya-capture --example event_tap -- --raw-ts`

This is an open check on #336. `--raw-ts` runs its own minimal tap for
10 s. For each event it prints a line like this:

```text
leftMouseDown raw=4410337711250 reading=ticks ts=183764071302083 latency_us=412 (as_ns_us=179353733 / as_ticks_us=412)
```

- `raw` is the unmodified `CGEventGetTimestamp`.
- `reading` is the interpretation `event_timestamp` picked: `ticks` (mach
  absolute time), `ns`, or `same` when both readings are equal (Intel, or
  a 1:1 timebase).
- `latency_us` is now minus the picked `ts`. The two values in brackets
  are the latency under each reading, so the wrong one is obviously off.

At the end it prints a summary with counts per reading and the maximum
latency.

- [ ] Use **real hardware**: a trackpad or mouse click and a keyboard press
      in another app. Note `reading` for each kind. On Apple silicon,
      hardware events are expected to read as `ticks`; on Intel, `same`.
- [ ] `latency_us` is small and positive: under a few ms, and never
      negative by more than a few µs. If the picked reading has a large
      latency and the other reading a small one, the picker is wrong:
      report it on #336.
- [ ] Optional: send a synthetic event (for example with a small
      `CGEventPost` tool or an automation app) and note its `reading`.
      It is expected to be `ns`.

## 2. Frames: `cargo run -p guidesya-capture --example frames [fps]`

Captures the main display for 3 s and prints one line per frame. The default
fps is `DEFAULT_FPS`; also try `frames 30`.

- [ ] Without Screen Recording it prints `could not start capture: …` and
      exits with status 1. It checks the permission without prompting, so
      grant it through the Settings link above.
- [ ] The first line describes the main display: size in points, origin,
      scale, and size in pixels. Pixels must equal points × scale. On a
      Retina display the scale is 2.
- [ ] Each frame's size in pixels matches the display, and
      `bytes` = width × height × 4.
- [ ] **Static screen:** keep the mouse still. You get only a few frames,
      which is expected. ScreenCaptureKit skips frames while nothing changes.
- [ ] **Busy screen:** play a video or move a window while it runs. The
      example keeps at most the **last 40 frames** and lists only those, so:
  - At the default 12 fps, 3 s is about 36 frames. Expect close to 36,
    with `+… ms` gaps around 83 ms.
  - At 30 fps, expect exactly 40 frames covering only the last ~1.3 s,
    with gaps around 33 ms. Fewer than fps × 3 frames is not a failure
    here.
- [ ] The `from start` offsets are increasing. At 13 fps or less they start
      near 0 ms. Above that, the first listed frame is later (about +1.7 s
      at 30 fps), because earlier frames were dropped from the buffer. Frame
      `ts` uses the same clock as event `ts` (`event_tap::now()`), which the
      step detector depends on to match clicks to frames.
- [ ] No `stream error:` line and no `stop failed:` line.
- [ ] Watch memory in Activity Monitor. With 40 frames held it should stay
      around 40 × frame size (about 830 MB on a 2880×1800 display) and fall
      back when the example exits.
- [ ] **CPU** (open check on #336): with `top -pid <pid>` or Activity
      Monitor, note the CPU % at the default fps and at 30 fps, with a static
      screen and with a busy one.
- [ ] **Multiple displays** (if you have them): only the main display is
      captured. Note what happens when the menu bar display changes during
      a run.

Window capture is not in this example. When it is wired into the session,
check that moving the window during a recording still puts clicks on the
right pixels: the origin is fixed at start/resume today, so it doesn't yet
(see #336).

## 3. Accessibility: `cargo run -p guidesya-capture --example ax`

Prints, once a second for 10 s, the AX element under the mouse and the
focused element, with how long the lookups took.

- [ ] Without Accessibility it prints `Accessibility is …; grant it to this
      terminal first.` Then the lookups return errors or nothing, and it
      still exits normally after 10 s.
- [ ] Hover over a button in Finder or System Settings. `under:` shows
      role `AXButton`, a useful `label` or `title`, the `window_title`, and
      the app (pid, name, bundle id). `frame` encloses the mouse position.
- [ ] Hover over the Dock, the menu bar, and an empty desktop. Each gives a
      sensible element or `None`, and nothing hangs.
- [ ] Hover over an Electron app (VS Code, Slack) and a web page in Safari.
      Note how good the labels are there.
- [ ] Lookup time stays well under the 1 s tick, normally a few ms. Note
      the worst case you see, and which app caused it.
- [ ] **Secure fields:** click into a password field (Safari login form, or
      Users & Groups in System Settings). `focused:` shows `is_secure: true`
      and `mask keys: true`. (This example prints Rust `Debug` output, so
      field names are snake_case.)
- [ ] Click into a normal text field. `mask keys: false`.
- [ ] Focus something that times out or has no focused element. `mask
      keys` is `true`: masking fails closed.

## 4. Record: `cargo run --release -p guidesya-capture --example record -- --seconds 20 [--raw DIR]`

This runs the M0 recording session: tap, frames and AX together. It needs
all three permissions. Use `--release`: snapshot frames are encoded to
WebP while recording, and a debug build encodes too slowly to keep up
(see the `encoder:` line in the output). It records for `--seconds` (default 20) and then
writes a `.guidesya` bundle to `--library` (default
`target/record/library`). It prints:

- the bundle path;
- the steps (kind, whether a crop was written, and the title);
- event counts (kept, dropped because they targeted our own pid, excluded);
- snapshots, and how many events found no earlier frame;
- the frame-to-event latency distribution (event ts minus the ts of the
  frame used, in ms);
- AX lookup counts and times;
- process CPU time over the session.

With `--raw DIR` it also writes:

- `events.jsonl`: the enriched events;
- `snapshots.jsonl`: event ts, frame ts and latency for each snapshot;
- `drafts.json`: the detected draft steps;
- `frames/<frame ts>.webp`: the snapshot frames.

Other options:

- `--title TEXT` sets the guide title.
- `--fps N` sets the capture rate.
- `--exclude BUNDLE_ID` drops one app's events. It can be repeated.

Suggested script for one run, with the Finder and TextEdit open:

1. Click a Finder sidebar item.
2. Double-click a folder.
3. Right-click a file.
4. Drag a file a short way.
5. In TextEdit, type a sentence and press Return.
6. Press ⌘S.
7. Press ⌃⌥S.
8. In a Safari login page, type into the username field, press Tab, then
   type into the password field.

- [ ] Without a permission, it prints a `warning:` line for it.
      If Screen Recording or the tap is refused, it prints `error: could
      not start recording: …` and exits with status 1. It must not hang.
- [ ] Each action above gives one step with a sensible title, for example
      "Click **Documents** in Finder", "Type `…` into …", "Press **⌘S**",
      a drag with an end point, and a manual step.
- [ ] **Manual hotkey:** ⌃⌥S gives a `Manual` step and **no** "Press
      ⌃⌥S" step.
- [ ] **Elements:** in `events.jsonl`, `mouseDown` has `element` filled
      (role, label or title, app) for normal controls.
- [ ] **Secure fields:** the password step's keystrokes are `••••••••`. In
      `events.jsonl`, the password's `keyDown` events have `"secure":true`,
      `characters` is `"•"`, and `charactersIgnoringModifiers` is absent.
      Grep the raw dump for the password: it must not appear anywhere. The
      username typed before Tab stays plain.
- [ ] **Fast Tab into a password field:** type the username, press Tab and
      type the password **immediately**. Grep `events.jsonl` for each
      password character: none may appear. The `masking:` line counts the
      keys held by the settle guard (typed within 300 ms of Tab or a click,
      before focus moved) and those scrubbed after the fact.
- [ ] **Masking when the session falls behind:** keys whose focus lookup
      ran more than 75 ms late are masked too, and counted in the "keys
      masked because their focus lookup was late" line. Note that count.
      If it is high on a normal run, typing will look masked in the guide.
      This check compares event `ts` with the host clock, so it is only
      trustworthy once the `--raw-ts` check in section 1b has passed.
- [ ] **Encoder:** the `encoder:` line shows the frames encoded, errors,
      snapshots lost, snapshots dropped because the queue (4 frames) was
      full, mean and max encode time, and the maximum backlog. In a
      release build expect no queue-full drops at a normal pace. Note the
      count after a burst of quick clicks.
- [ ] **Session folder:** snapshots are written to
      `~/Library/Caches/dev.guidesya.app/sessions/.guidesya-session-<ulid>`,
      never to the library. After the run, no `.guidesya-session-*` folder
      is left there or in the library. Kill a run with ⌃C partway through:
      its folder stays, and it is removed at a later start once it is more
      than 6 hours old.
- [ ] `focusChanged` and `appActivated` events appear when you move between
      fields and apps.
- [ ] **Exclusion:** run again with `--exclude com.apple.TextEdit`.
      TextEdit clicks and typing give no steps and count as excluded.
- [ ] **Own pid:** the `own-pid dropped` count is 0 for a terminal-only
      run. The example has no windows; check this again with the app's
      toolbar when it exists.
- [ ] **Frame-to-click accuracy:** open a few step images (`steps/<id>.webp`
      and `.crop.webp` in the bundle). Each must show the screen **before**
      the click: a menu you opened by that click is not open yet. **Judge
      accuracy by what the images show, not by the latency figure.**
      ScreenCaptureKit sends no frames while the screen is static, so on a
      quiet screen "event ts − frame ts" can be seconds even though the
      frame is exactly right.
- [ ] **Latency figure:** only meaningful while the screen is changing
      (play a video or scroll a page while you click). Then, at 12 fps,
      expect a median under ~85 ms. Note the median, p95 and max, and the
      number of "no earlier frame" events (expect few or none).
- [ ] The crops are centred on the click point and are 800×500 px, or
      smaller near the screen edges.
- [ ] **CPU:** note the CPU % printed at the default fps and at `--fps 30`,
      with a mostly static screen and with a busy one. This number is for
      the whole process.
- [ ] Move a window between clicks and confirm that the crops are still
      centred on the clicked control (display capture, so this should
      hold).
- [ ] Copy one `events.jsonl` from a good run into
      `crates/guidesya-core/testdata/traces/`, with the secure parts
      checked, as a step-detection fixture (the coordinator lands it).

## 5. Other integration checks from #336

- [ ] Clicking GuidesYa's own recording toolbar: check that `targetPid` is
      filled in, and that those clicks are left out of the guide (needs the
      app).
- [ ] **TCC survives rebuilds:** grant Screen Recording to the signed app
      bundle (see [signing.md](signing.md)), rebuild, launch again. The grant
      must still be there and capture must start without a new prompt.

## Results

Copy this block into the todo or PR you are testing for:

```text
Date / commit:
macOS / Mac / displays:
event_tap: pass/fail, targetPid ok?, ts unit (Δ for ~1 s), --raw-ts reading (mouse/key):
frames: pass/fail, fps default/30 (static, busy), CPU % default/30:
ax: pass/fail, worst lookup time (app), secure field masked?:
record: steps ok?, latency median/p95/max ms, CPU % default/30 fps, secure masked?:
#336 integration checks:
Notes:
```
