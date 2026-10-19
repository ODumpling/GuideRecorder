/// <reference types="vite/client" />
import { createRootRoute, HeadContent, Link, Outlet, Scripts } from "@tanstack/react-router";
import type { ReactNode } from "react";
import appCss from "@/styles.css?url";

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1" },
      { title: "GuideRecorder" },
      { name: "description", content: "Share, review and sync your GuideRecorder guides." },
    ],
    links: [
      { rel: "stylesheet", href: appCss },
      { rel: "icon", href: "/favicon.ico" },
    ],
  }),
  component: Outlet,
  shellComponent: Document,
  notFoundComponent: () => (
    <main className="mx-auto grid max-w-md flex-1 place-content-center gap-3 p-8 text-center">
      <h1 className="font-serif text-4xl">Not found</h1>
      <Link to="/" className="text-brand underline-offset-4 hover:underline">
        Go home
      </Link>
    </main>
  ),
});

function Document({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className="h-full antialiased">
      <head>
        <HeadContent />
      </head>
      <body className="flex min-h-full flex-col font-sans">
        {children}
        <Scripts />
      </body>
    </html>
  );
}
