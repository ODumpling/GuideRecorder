import { Link } from "@tanstack/react-router";
import { useEffect, useState, type ReactNode } from "react";
import {
  AppWindow,
  ArrowRight,
  BookOpen,
  CalendarClock,
  Check,
  CheckCircle2,
  Clapperboard,
  Download,
  EyeOff,
  FileText,
  Folder,
  HardDrive,
  KeyRound,
  Link2,
  Lock,
  MousePointer2,
  Pause,
  PenLine,
  Plus,
  ScrollText,
  ShieldCheck,
  Sparkles,
  Square,
  Terminal,
  Users,
} from "lucide-react";
import { cn } from "@guiderecorder/ui/utils";

// The public front page. Dark, on its own palette rather than the app's
// theme; the product shots are drawn in markup, not screenshots, so they
// stay sharp and follow the real app's layout.

const REPO = "https://github.com/ODumpling/GuideRecorder";
const DOWNLOAD = `${REPO}/releases/latest`;

export function Landing({ signedIn }: { signedIn: boolean }) {
  return (
    <div className="relative flex-1 overflow-x-clip bg-[#06080f] text-slate-100 selection:bg-blue-500/40">
      <div aria-hidden className="pointer-events-none absolute inset-0 bg-[linear-gradient(to_right,rgb(255_255_255/0.035)_1px,transparent_1px),linear-gradient(to_bottom,rgb(255_255_255/0.035)_1px,transparent_1px)] bg-[size:64px_64px] [mask-image:radial-gradient(ellipse_at_top,black_20%,transparent_70%)]" />
      <Nav signedIn={signedIn} />
      <Hero signedIn={signedIn} />
      <HowItWorks />
      <Privacy />
      <Exports />
      <Teams />
      <Developers />
      <Closing signedIn={signedIn} />
      <Footer />
    </div>
  );
}

function Nav({ signedIn }: { signedIn: boolean }) {
  return (
    <header className="relative z-10 mx-auto flex h-16 w-full max-w-6xl items-center gap-6 px-6">
      <Link to="/" className="flex items-center gap-2 font-medium tracking-tight">
        <Mark className="size-6" />
        GuideRecorder
      </Link>
      <nav className="flex flex-1 items-center justify-center gap-6 text-sm text-slate-400 max-md:hidden">
        <a href="#how" className="hover:text-white">How it works</a>
        <a href="#privacy" className="hover:text-white">Privacy</a>
        <a href="#export" className="hover:text-white">Export</a>
        <a href="#teams" className="hover:text-white">Teams</a>
        <a href="#developers" className="hover:text-white">Developers</a>
      </nav>
      <div className="flex items-center gap-2 max-md:ml-auto">
        <a href={REPO} className="flex h-8 items-center gap-1.5 rounded-full border border-white/10 px-3 text-xs text-slate-300 hover:border-white/20 hover:text-white">
          <GitHubIcon className="size-3.5" />
          <span className="max-sm:hidden">GitHub</span>
        </a>
        {signedIn ? (
          <Pill to="/library">Open library</Pill>
        ) : (
          <>
            <Link to="/sign-in" className="px-2 text-sm whitespace-nowrap text-slate-300 hover:text-white max-sm:hidden">
              Sign in
            </Link>
            <Pill to="/sign-up">Get started</Pill>
          </>
        )}
      </div>
    </header>
  );
}

function Pill({ to, children }: { to: "/library" | "/sign-up"; children: ReactNode }) {
  return (
    <Link to={to} className="flex h-8 items-center rounded-full bg-white px-3.5 text-sm font-medium whitespace-nowrap text-slate-950 hover:bg-slate-200">
      {children}
    </Link>
  );
}

function Hero({ signedIn }: { signedIn: boolean }) {
  return (
    <section className="relative px-6 pt-14 pb-24 text-center sm:pt-20">
      <div aria-hidden className="pointer-events-none absolute top-0 left-1/2 h-[520px] w-[900px] -translate-x-1/2 bg-[radial-gradient(closest-side,rgb(37_99_235/0.28),transparent)]" />
      <h1 className="relative mx-auto max-w-4xl animate-landing-rise text-5xl font-semibold tracking-[-0.04em] text-balance sm:text-7xl">
        Do it once. <span className="font-serif font-normal tracking-normal text-blue-300 italic">Get the guide.</span>
      </h1>
      <p className="relative mx-auto mt-6 max-w-xl animate-landing-rise text-lg text-pretty text-slate-400 [animation-delay:80ms]">
        GuideRecorder watches you do a task on your computer and writes the how-to: one step per click, each with its screenshot. Tidy it up, then
        share it as a PDF, a doc, a video or a link.
      </p>
      <div className="relative mt-9 flex animate-landing-rise flex-wrap items-center justify-center gap-3 [animation-delay:160ms]">
        <a href={DOWNLOAD} className="flex h-11 items-center gap-2 rounded-full bg-white px-5 font-medium text-slate-950 shadow-[0_0_40px_rgb(59_130_246/0.35)] hover:bg-slate-200">
          <AppleIcon className="size-4" />
          Download for macOS
        </a>
        <a href={DOWNLOAD} className="flex h-11 items-center gap-2 rounded-full border border-white/15 bg-white/5 px-5 font-medium text-white hover:bg-white/10">
          <WindowsIcon className="size-4" />
          Download for Windows
          <span className="rounded-full bg-amber-400/15 px-2 py-0.5 text-[11px] font-medium text-amber-300">Alpha</span>
        </a>
        <Link
          to={signedIn ? "/library" : "/sign-up"}
          className="flex h-11 items-center gap-2 rounded-full border border-white/15 bg-white/5 px-5 font-medium text-white hover:bg-white/10"
        >
          {signedIn ? "Open your library" : "Create a team workspace"}
          <ArrowRight className="size-4" />
        </Link>
      </div>
      <p className="relative mt-5 text-xs text-slate-500">Free and open source · macOS 12.3+ · Windows 10 (2004+) and 11, in alpha</p>
      <div className="relative mx-auto mt-14 max-w-5xl animate-landing-rise [animation-delay:260ms] [perspective:2000px]">
        <AppMock />
      </div>
    </section>
  );
}

const STEPS = [
  { title: "Open **Settings**", app: "Acme Console" },
  { title: "Click **API keys**", app: "Acme Console" },
  { title: "Click **Rotate key**", app: "Acme Console" },
  { title: "Type a name for the new key", app: "Acme Console" },
  { title: "Click **Copy** and save it", app: "Acme Console" },
];

/** The desktop editor mid-recording: library, steps, the step's screenshot, and the floating toolbar. */
function AppMock() {
  const seconds = useTicker(42);
  const shown = Math.min(STEPS.length, 3 + Math.floor((seconds - 42) / 3));
  return (
    <div className="relative">
      <div className="overflow-hidden rounded-xl border border-white/10 bg-[#0c111c] text-left shadow-[0_40px_120px_-20px_rgb(37_99_235/0.35)] [transform:rotateX(8deg)] [transform-origin:top]">
        <div className="flex h-9 items-center gap-2 border-b border-white/5 px-3">
          <span className="size-2.5 rounded-full bg-[#ff5f57]" />
          <span className="size-2.5 rounded-full bg-[#febc2e]" />
          <span className="size-2.5 rounded-full bg-[#28c840]" />
          <span className="ml-3 text-xs text-slate-500">Rotate the production API key</span>
        </div>
        <div className="grid h-[440px] grid-cols-[180px_240px_1fr] text-[13px] max-lg:grid-cols-[240px_1fr] max-sm:grid-cols-1">
          <aside className="flex flex-col gap-4 border-r border-white/5 bg-white/[0.02] p-3 text-slate-400 max-lg:hidden">
            <div className="flex flex-col gap-0.5">
              <SideItem icon={<BookOpen />} label="All guides" count={48} active />
              <SideItem icon={<CalendarClock />} label="Review due" count={3} />
            </div>
            <div className="flex flex-col gap-0.5">
              <p className="px-2 text-[10px] font-medium tracking-wider text-slate-600 uppercase">Status</p>
              <SideItem icon={<Dot className="bg-slate-400" />} label="Draft" count={9} />
              <SideItem icon={<Dot className="bg-amber-400" />} label="In review" count={4} />
              <SideItem icon={<Dot className="bg-emerald-400" />} label="Published" count={35} />
            </div>
            <div className="flex flex-col gap-0.5">
              <p className="px-2 text-[10px] font-medium tracking-wider text-slate-600 uppercase">Folders</p>
              <SideItem icon={<Folder />} label="Onboarding" count={12} />
              <SideItem icon={<Folder />} label="Engineering" count={21} />
              <SideItem icon={<Folder />} label="Support" count={15} />
            </div>
          </aside>
          <ol className="flex flex-col gap-1 overflow-hidden border-r border-white/5 p-2 max-sm:hidden">
            {STEPS.slice(0, shown).map((step, i) => (
              <li
                key={step.title}
                className={cn(
                  "flex animate-landing-rise gap-2.5 rounded-lg p-2",
                  i === 2 ? "bg-blue-500/15 ring-1 ring-blue-400/30" : "hover:bg-white/5",
                )}
              >
                <span className={cn("grid size-5 shrink-0 place-items-center rounded-full text-[10px] font-semibold", i === 2 ? "bg-blue-500 text-white" : "bg-white/10 text-slate-300")}>
                  {i + 1}
                </span>
                <div className="min-w-0">
                  <p className="truncate text-slate-200">
                    <Bold text={step.title} />
                  </p>
                  <div className="mt-1.5 h-10 rounded border border-white/5 bg-gradient-to-br from-slate-800 to-slate-900" />
                </div>
              </li>
            ))}
          </ol>
          <div className="flex min-w-0 flex-col gap-3 p-5">
            <div className="flex items-center gap-2">
              <span className="grid size-6 place-items-center rounded-full bg-blue-500 text-xs font-semibold text-white">3</span>
              <p className="text-base font-medium text-white">
                Click <strong>Rotate key</strong>
              </p>
              <span className="ml-auto flex items-center gap-1 rounded-full bg-cyan-400/10 px-2 py-0.5 text-[11px] text-cyan-300">
                <Sparkles className="size-3" /> Write with AI
              </span>
            </div>
            <p className="text-slate-400">Next to the key you want to replace, click Rotate key. The old key keeps working for 24 hours.</p>
            <FakeScreenshot />
          </div>
        </div>
      </div>
      <div className="absolute -bottom-6 left-1/2 flex -translate-x-1/2 items-center gap-3 rounded-full border border-white/10 bg-[#111827]/95 py-1.5 pr-1.5 pl-4 text-sm shadow-2xl backdrop-blur">
        <span className="size-2.5 animate-pulse rounded-full bg-red-500" />
        <span className="font-mono text-slate-200 tabular-nums">{clock(seconds)}</span>
        <span className="text-slate-500">·</span>
        <span className="text-slate-300 tabular-nums">{shown} steps</span>
        <span className="flex gap-1">
          <ToolbarButton label="Pause"><Pause /></ToolbarButton>
          <ToolbarButton label="Add step"><Plus /></ToolbarButton>
          <ToolbarButton label="Stop" stop><Square /></ToolbarButton>
        </span>
      </div>
    </div>
  );
}

/** The screenshot of the step: a settings page, the click, a callout and a blur. */
function FakeScreenshot() {
  return (
    <div className="relative min-h-0 flex-1 overflow-hidden rounded-lg border border-white/10 bg-[#f3f6fa] text-slate-700">
      <div className="flex h-7 items-center gap-2 border-b border-slate-200 bg-white px-3 text-[11px] text-slate-500">
        <span className="font-semibold text-slate-800">Acme Console</span>
        <span>/ Settings / API keys</span>
      </div>
      <div className="flex flex-col gap-2 p-4">
        {[
          { name: "Production", key: "sk_live_8f2c…", hot: true },
          { name: "Staging", key: "sk_test_31aa…", hot: false },
          { name: "Analytics", key: "sk_live_c07e…", hot: false },
          { name: "CI deploys", key: "sk_test_9b4d…", hot: false },
        ].map((k) => (
          <div key={k.name} className="flex items-center gap-3 rounded-md border border-slate-200 bg-white px-3 py-2 text-[11px]">
            <KeyRound className="size-3.5 text-slate-400" />
            <span className="font-medium text-slate-800">{k.name}</span>
            {k.hot ? (
              <span className="rounded bg-slate-300 px-1.5 text-transparent blur-[3px] select-none">{k.key}</span>
            ) : (
              <span className="font-mono text-slate-400">{k.key}</span>
            )}
            <span
              className={cn(
                "relative ml-auto rounded px-2 py-0.5 font-medium",
                k.hot ? "bg-blue-600 text-white ring-2 ring-orange-400 ring-offset-2" : "border border-slate-200 text-slate-500",
              )}
            >
              Rotate key
              {k.hot ? (
                <>
                  <span className="absolute top-1/2 left-1/2 size-8 -translate-1/2 animate-landing-ping rounded-full border-2 border-orange-400" />
                  <MousePointer2 className="absolute top-3 left-8 size-4 fill-slate-900 text-white" />
                </>
              ) : null}
            </span>
          </div>
        ))}
      </div>
      <span className="absolute right-4 bottom-3 rounded-md bg-orange-400 px-2 py-1 text-[10px] font-medium text-slate-950 shadow">Old key stays valid for 24h</span>
      <span className="absolute bottom-3 left-4 flex items-center gap-1 rounded-md bg-slate-900/85 px-2 py-1 text-[10px] text-white">
        <EyeOff className="size-3" /> API key blurred
      </span>
    </div>
  );
}

function HowItWorks() {
  const steps = [
    {
      icon: <span className="size-3 rounded-full bg-red-500 shadow-[0_0_12px_rgb(239_68_68)]" />,
      title: "Record",
      body: "Press ⌃⌥R and do the task. Each click, shortcut and typed text becomes a step with its screenshot. Whole screen, one window or a region.",
    },
    {
      icon: <MousePointer2 className="size-4 text-blue-300" />,
      title: "Edit",
      body: "Reword, reorder, merge and split steps. Annotate with arrows, boxes, callouts and blurs. Undo anything; every version is kept.",
    },
    {
      icon: <Sparkles className="size-4 text-cyan-300" />,
      title: "Let AI write it",
      body: "Optional. Turns raw steps into clear instructions with a title, intro and summary, and never overwrites what you wrote.",
    },
    {
      icon: <Download className="size-4 text-emerald-300" />,
      title: "Share",
      body: "Export to PDF, Word, Markdown, HTML, slides, video or GIF. Or sync to the web and send a link.",
    },
  ];
  return (
    <Section id="how" eyebrow="How it works" title="From a task you did to a guide anyone can follow.">
      <ol className="grid gap-px overflow-hidden rounded-2xl border border-white/10 bg-white/10 sm:grid-cols-2 lg:grid-cols-4">
        {steps.map((s, i) => (
          <li key={s.title} className="flex flex-col gap-3 bg-[#0a0e18] p-6">
            <div className="flex items-center justify-between">
              <span className="grid size-9 place-items-center rounded-lg border border-white/10 bg-white/5">{s.icon}</span>
              <span className="font-mono text-xs text-slate-600">0{i + 1}</span>
            </div>
            <h3 className="mt-2 text-lg font-medium text-white">{s.title}</h3>
            <p className="text-sm leading-relaxed text-slate-400">{s.body}</p>
          </li>
        ))}
      </ol>
    </Section>
  );
}

function Privacy() {
  const points = [
    { icon: <HardDrive />, title: "Local-first", body: "Guides are plain folders on your disk. Nothing is uploaded unless you turn on sync." },
    { icon: <Lock />, title: "Passwords never recorded", body: "Keystrokes in secure fields are masked before they're stored." },
    { icon: <AppWindow />, title: "Excluded apps", body: "1Password, Messages and the like are kept out of every screenshot, even behind other windows." },
    { icon: <Sparkles />, title: "AI only when you say so", body: "Bring your own Anthropic or OpenAI-compatible key, or run a local model in Ollama or LM Studio." },
  ];
  return (
    <Section id="privacy" eyebrow="Private by default" title="Safe to leave running.">
      <div className="grid gap-6 lg:grid-cols-[1.1fr_1fr]">
        <Card className="flex flex-col gap-5 p-6">
          <div>
            <h3 className="text-lg font-medium text-white">Sensitive text is found on your computer</h3>
            <p className="mt-1 text-sm text-slate-400">
              When you stop recording, the text in every screenshot is read on the device and checked for emails, card numbers, IBANs, API keys
              and your own patterns. Each finding becomes a blur you keep or dismiss, and blurs are burned into the pixels on export.
            </p>
          </div>
          <div className="flex flex-col gap-2 rounded-xl border border-white/10 bg-black/30 p-3 text-sm">
            {[
              { kind: "Email", text: "jamie@acme-corp.com", step: 2 },
              { kind: "API key", text: "sk_live_8f2c41d09ab7", step: 3 },
              { kind: "Card number", text: "4242 4242 4242 4242", step: 6 },
            ].map((f, i) => (
              <div key={f.kind} className="flex items-center gap-3 rounded-lg bg-white/[0.03] px-3 py-2">
                <EyeOff className="size-4 shrink-0 text-slate-500" />
                <span className="w-24 shrink-0 text-slate-300">{f.kind}</span>
                <span className="min-w-0 truncate rounded bg-slate-500/30 px-1.5 font-mono text-xs text-slate-300 blur-[3px] select-none">{f.text}</span>
                <span className="ml-auto shrink-0 text-xs text-slate-500 max-sm:hidden">Step {f.step}</span>
                <span
                  className={cn(
                    "shrink-0 rounded-md px-2 py-0.5 text-xs",
                    i === 2 ? "border border-white/10 text-slate-300" : "bg-emerald-500/15 text-emerald-300",
                  )}
                >
                  {i === 2 ? "Blur" : "Blurred"}
                </span>
              </div>
            ))}
          </div>
        </Card>
        <div className="grid gap-px overflow-hidden rounded-2xl border border-white/10 bg-white/10 sm:grid-cols-2">
          {points.map((p) => (
            <div key={p.title} className="flex flex-col gap-2 bg-[#0a0e18] p-5 [&_svg]:size-4 [&_svg]:text-blue-300">
              {p.icon}
              <h3 className="mt-1 font-medium text-white">{p.title}</h3>
              <p className="text-sm text-slate-400">{p.body}</p>
            </div>
          ))}
        </div>
      </div>
    </Section>
  );
}

function Exports() {
  const formats = ["pdf", "docx", "md", "html", "pptx", "mp4", "gif"];
  return (
    <Section id="export" eyebrow="Export" title="One guide, every format your team reads.">
      <div className="grid gap-6 lg:grid-cols-2">
        <Card className="flex flex-col gap-5 p-6">
          <div>
            <h3 className="flex items-center gap-2 text-lg font-medium text-white">
              <FileText className="size-4 text-blue-300" /> Documents
            </h3>
            <p className="mt-1 text-sm text-slate-400">
              PDF and Word on A4 or Letter, Markdown, HTML, and slides with one step per slide. Your logo and colour, blurs applied to the actual
              pixels.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            {formats.map((f) => (
              <span key={f} className="rounded-full border border-white/10 bg-white/5 px-3 py-1 font-mono text-xs text-slate-300">
                .{f}
              </span>
            ))}
          </div>
          <div className="mt-auto grid grid-cols-3 gap-3">
            {[0, 1, 2].map((i) => (
              <div key={i} className="aspect-[3/4] rounded-md bg-white p-2 shadow-lg" style={{ transform: `rotate(${(i - 1) * 3}deg)` }}>
                <div className="h-1.5 w-2/3 rounded bg-slate-800" />
                <div className="mt-1.5 h-1 w-full rounded bg-slate-200" />
                <div className="mt-1 h-1 w-4/5 rounded bg-slate-200" />
                <div className="mt-2 h-1/3 rounded bg-gradient-to-br from-blue-100 to-slate-200" />
                <div className="mt-2 h-1 w-full rounded bg-slate-200" />
                <div className="mt-1 h-1 w-3/5 rounded bg-slate-200" />
              </div>
            ))}
          </div>
        </Card>
        <Card className="flex flex-col gap-5 p-6">
          <div>
            <h3 className="flex items-center gap-2 text-lg font-medium text-white">
              <Clapperboard className="size-4 text-blue-300" /> Video and GIF
            </h3>
            <p className="mt-1 text-sm text-slate-400">
              For a chat, a ticket or a README: each step zooms smoothly to where you clicked, with a cursor, a click marker and a caption. Wide,
              square or vertical.
            </p>
          </div>
          <div className="relative mt-auto aspect-video overflow-hidden rounded-xl border border-white/10 bg-[#f3f6fa]">
            <div className="absolute inset-0 origin-[70%_45%] animate-landing-zoom">
              <div className="flex h-6 items-center border-b border-slate-200 bg-white px-3 text-[10px] font-semibold text-slate-800">Acme Console</div>
              <div className="absolute top-[38%] left-[58%] rounded bg-blue-600 px-2 py-0.5 text-[10px] font-medium text-white">Rotate key</div>
              <span className="absolute top-[36%] left-[64%] size-6 animate-landing-ping rounded-full border-2 border-orange-400" />
              <MousePointer2 className="absolute top-[42%] left-[66%] size-4 fill-slate-900 text-white" />
            </div>
            <div className="absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/80 to-transparent px-4 pt-6 pb-3 text-center text-sm font-medium text-white">
              3. Click Rotate key
            </div>
          </div>
        </Card>
      </div>
    </Section>
  );
}

function Teams() {
  const features = [
    { icon: <Users />, title: "One library for the team", body: "Sync guides from the app to a shared workspace. Edits on either side merge; clashes ask which to keep." },
    { icon: <Link2 />, title: "Share with a link", body: "Public or unlisted links, with passwords and expiry when your admins want them." },
    { icon: <CheckCircle2 />, title: "Review and approval", body: "Guides move from draft to in review to published, with owners and review dates so they don't go stale." },
    { icon: <ScrollText />, title: "Audit log", body: "Who changed, approved, shared or exported what, and when." },
    { icon: <ShieldCheck />, title: "SSO and SCIM", body: "Sign in with your identity provider and provision members automatically." },
    { icon: <PenLine />, title: "Edit on the web", body: "Fix a typo or a step from any browser, without the desktop app." },
  ];
  return (
    <Section id="teams" eyebrow="For teams" title="Keep the whole team's know-how current.">
      <div className="grid gap-6 lg:grid-cols-[1fr_1.15fr]">
        <div className="grid content-start gap-x-6 gap-y-7 sm:grid-cols-2">
          {features.map((f) => (
            <div key={f.title} className="flex flex-col gap-1.5 [&_svg]:size-4 [&_svg]:text-blue-300">
              {f.icon}
              <h3 className="mt-1 font-medium text-white">{f.title}</h3>
              <p className="text-sm text-slate-400">{f.body}</p>
            </div>
          ))}
        </div>
        <Card className="flex flex-col gap-3 p-4 text-sm">
          <div className="flex items-center justify-between px-1 text-xs text-slate-500">
            <span>Engineering</span>
            <span>Synced just now</span>
          </div>
          {[
            { title: "Rotate the production API key", status: "In review", color: "bg-amber-400", steps: 7, who: "JM" },
            { title: "Deploy a hotfix to staging", status: "Published", color: "bg-emerald-400", steps: 12, who: "AK" },
            { title: "Add a teammate to the VPN", status: "Published", color: "bg-emerald-400", steps: 9, who: "RS" },
            { title: "Restore a database snapshot", status: "Draft", color: "bg-slate-400", steps: 15, who: "JM" },
          ].map((g, i) => (
            <div key={g.title} className={cn("flex items-center gap-3 rounded-lg border px-3 py-2.5", i === 0 ? "border-blue-400/30 bg-blue-500/10" : "border-white/5 bg-white/[0.02]")}>
              <div className="h-9 w-14 shrink-0 rounded border border-white/10 bg-gradient-to-br from-slate-700 to-slate-900" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-slate-100">{g.title}</p>
                <p className="flex items-center gap-1.5 text-xs text-slate-500">
                  <span className={cn("size-1.5 rounded-full", g.color)} />
                  {g.status} · {g.steps} steps
                </p>
              </div>
              {i === 0 ? (
                <span className="flex shrink-0 items-center gap-1 rounded-md bg-white px-2 py-1 text-xs font-medium text-slate-950">
                  <Check className="size-3" /> Approve
                </span>
              ) : (
                <span className="grid size-7 shrink-0 place-items-center rounded-full bg-white/10 text-[10px] text-slate-300">{g.who}</span>
              )}
            </div>
          ))}
        </Card>
      </div>
    </Section>
  );
}

function Developers() {
  return (
    <Section id="developers" eyebrow="For developers" title="Your guides, from Claude and the command line.">
      <div className="grid gap-6 lg:grid-cols-[1fr_1.3fr] lg:items-center">
        <div className="flex flex-col gap-4 text-slate-400">
          <p>
            A read-only <span className="text-slate-200">MCP server</span> lets Claude and other agents list, search and read your guides, so “how
            do we rotate the API key?” gets your team’s answer.
          </p>
          <p>
            The <span className="text-slate-200">CLI</span> lists, searches, validates and exports guides in every format, for scripts and CI.
          </p>
          <a href={REPO} className="mt-2 flex items-center gap-1.5 text-sm text-blue-300 hover:text-blue-200">
            Read the docs on GitHub <ArrowRight className="size-3.5" />
          </a>
        </div>
        <Card className="overflow-hidden font-mono text-[13px]">
          <div className="flex h-9 items-center gap-2 border-b border-white/5 px-3 text-xs text-slate-500">
            <Terminal className="size-3.5" /> ~/GuideRecorder
          </div>
          <pre className="overflow-x-auto p-4 leading-relaxed text-slate-300">
            <Line prompt>claude mcp add guiderecorder -- guiderecorder-mcp</Line>
            <Line ok>Added stdio MCP server guiderecorder</Line>
            <Line> </Line>
            <Line prompt>guiderecorder search "api key"</Line>
            <Line dim>Rotate the production API key   7 steps  Engineering</Line>
            <Line dim>Create a read-only API key      5 steps  Engineering</Line>
            <Line> </Line>
            <Line prompt>guiderecorder export rotate-key.guiderecorder --format mp4</Line>
            <Line ok>Wrote rotate-key.mp4 (7 steps, 0:28)</Line>
          </pre>
        </Card>
      </div>
    </Section>
  );
}

function Closing({ signedIn }: { signedIn: boolean }) {
  return (
    <section className="relative px-6 py-28 text-center">
      <div aria-hidden className="pointer-events-none absolute inset-x-0 top-1/2 h-80 -translate-y-1/2 bg-[radial-gradient(closest-side,rgb(37_99_235/0.22),transparent)]" />
      <h2 className="relative mx-auto max-w-2xl text-4xl font-semibold tracking-[-0.03em] text-balance sm:text-5xl">
        The next time someone asks <span className="font-serif font-normal tracking-normal text-blue-300 italic">“how do I…?”</span>
      </h2>
      <p className="relative mx-auto mt-5 max-w-md text-slate-400">Send them a guide. GuideRecorder is free, open source and MIT licensed.</p>
      <div className="relative mt-8 flex flex-wrap justify-center gap-3">
        <a href={DOWNLOAD} className="flex h-11 items-center gap-2 rounded-full bg-white px-5 font-medium text-slate-950 hover:bg-slate-200">
          <AppleIcon className="size-4" />
          Download for macOS
        </a>
        <a href={DOWNLOAD} className="flex h-11 items-center gap-2 rounded-full border border-white/15 bg-white/5 px-5 font-medium text-white hover:bg-white/10">
          <WindowsIcon className="size-4" />
          Download for Windows
          <span className="rounded-full bg-amber-400/15 px-2 py-0.5 text-[11px] font-medium text-amber-300">Alpha</span>
        </a>
        <a href={REPO} className="flex h-11 items-center gap-2 rounded-full border border-white/15 bg-white/5 px-5 font-medium hover:bg-white/10">
          <GitHubIcon className="size-4" />
          Star on GitHub
        </a>
      </div>
      {signedIn ? null : (
        <p className="relative mt-5 text-sm text-slate-500">
          Already on a team?{" "}
          <Link to="/sign-in" className="text-slate-300 underline-offset-4 hover:underline">
            Sign in
          </Link>
        </p>
      )}
    </section>
  );
}

function Footer() {
  return (
    <footer className="relative border-t border-white/5">
      <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-x-6 gap-y-3 px-6 py-8 text-sm text-slate-500">
        <span className="flex items-center gap-2 text-slate-300">
          <Mark className="size-5" /> GuideRecorder
        </span>
        <span>MIT licensed</span>
        <span className="flex-1" />
        <a href={REPO} className="hover:text-slate-300">GitHub</a>
        <a href={DOWNLOAD} className="hover:text-slate-300">Releases</a>
        <a href={`${REPO}/issues`} className="hover:text-slate-300">Issues</a>
        <Link to="/sign-in" className="hover:text-slate-300">Sign in</Link>
      </div>
    </footer>
  );
}

// --- Bits

function Section({ id, eyebrow, title, children }: { id: string; eyebrow: string; title: string; children: ReactNode }) {
  return (
    <section id={id} className="relative mx-auto w-full max-w-6xl scroll-mt-8 px-6 py-20">
      <p className="text-xs font-medium tracking-[0.2em] text-blue-300 uppercase">{eyebrow}</p>
      <h2 className="mt-3 mb-10 max-w-2xl text-3xl font-semibold tracking-[-0.03em] text-balance text-white sm:text-4xl">{title}</h2>
      {children}
    </section>
  );
}

function Card({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn("rounded-2xl border border-white/10 bg-gradient-to-b from-white/[0.05] to-white/[0.015]", className)}>{children}</div>;
}

function SideItem({ icon, label, count, active }: { icon: ReactNode; label: string; count: number; active?: boolean }) {
  return (
    <span className={cn("flex items-center gap-2 rounded-md px-2 py-1 [&_svg]:size-3.5", active && "bg-white/10 text-white")}>
      {icon}
      <span className="flex-1 truncate">{label}</span>
      <span className="text-[11px] text-slate-600 tabular-nums">{count}</span>
    </span>
  );
}

function Dot({ className }: { className: string }) {
  return <span className={cn("mx-[3px] size-2 rounded-full", className)} />;
}

function ToolbarButton({ label, stop, children }: { label: string; stop?: boolean; children: ReactNode }) {
  return (
    <span
      aria-label={label}
      className={cn("grid size-7 place-items-center rounded-full [&_svg]:size-3.5", stop ? "bg-red-500 text-white [&_svg]:fill-current" : "bg-white/10 text-slate-200")}
    >
      {children}
    </span>
  );
}

function Line({ prompt, ok, dim, children }: { prompt?: boolean; ok?: boolean; dim?: boolean; children: ReactNode }) {
  return (
    <div className={cn(ok && "text-emerald-300", dim && "text-slate-500")}>
      {prompt ? <span className="text-blue-300">$ </span> : null}
      {ok ? "✓ " : null}
      {children}
    </div>
  );
}

/** `**bold**` in a step title. */
function Bold({ text }: { text: string }) {
  return text.split("**").map((part, i) => (i % 2 ? <strong key={i} className="text-white">{part}</strong> : part));
}

/** Seconds since mount, starting at `from`; stays put for reduced motion. */
function useTicker(from: number) {
  const [seconds, setSeconds] = useState(from);
  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const id = window.setInterval(() => setSeconds((s) => s + 1), 1000);
    return () => window.clearInterval(id);
  }, []);
  return seconds;
}

const clock = (s: number) => `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;

function Mark({ className }: { className?: string }) {
  return (
    <span className={cn("grid place-items-center rounded-md bg-gradient-to-br from-blue-400 to-blue-700", className)}>
      <span className="size-1/3 rounded-full bg-white" />
    </span>
  );
}

function GitHubIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden className={className}>
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}

function WindowsIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 448 512" fill="currentColor" aria-hidden className={className}>
      <path d="M0 93.7l183.6-25.3v177.4H0V93.7zm0 324.6l183.6 25.3V268.4H0v149.9zm203.8 28L448 480V268.4H203.8v177.9zm0-380.6v180.1H448V32L203.8 65.7z" />
    </svg>
  );
}

function AppleIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 384 512" fill="currentColor" aria-hidden className={className}>
      <path d="M318.7 268.7c-.2-36.7 16.4-64.4 50-84.8-18.8-26.9-47.2-41.7-84.7-44.6-35.5-2.8-74.3 20.7-88.5 20.7-15 0-49.4-19.7-76.4-19.7C63.3 141.2 4 184.8 4 273.5q0 39.3 14.4 81.2c12.8 36.7 59 126.7 107.2 125.2 25.2-.6 43-17.9 75.8-17.9 31.8 0 48.3 17.9 76.4 17.9 48.6-.7 90.4-82.5 102.6-119.3-65.2-30.7-61.7-90-61.7-91.9zm-56.6-164.2c27.3-32.4 24.8-61.9 24-72.5-24.1 1.4-52 16.4-67.9 34.9-17.5 19.8-27.8 44.3-25.6 71.9 26.1 2 49.9-11.4 69.5-34.3z" />
    </svg>
  );
}
