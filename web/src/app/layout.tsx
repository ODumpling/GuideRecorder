import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: { default: "GuideRecorder", template: "%s · GuideRecorder" },
  description: "Share, review and sync your GuideRecorder guides.",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" className="h-full antialiased">
      <body className="flex min-h-full flex-col font-sans">{children}</body>
    </html>
  );
}
