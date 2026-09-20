import type { Metadata } from "next";
import Link from "next/link";
import "./globals.css";
import { Mark } from "@/lib/mark";

export const metadata: Metadata = {
  metadataBase: new URL("https://recuros.vercel.app"),
  title: {
    default: "RecurOS — one memory for every AI tool you use",
    template: "%s — RecurOS",
  },
  description:
    "RecurOS records the decisions, constraints and rejected options behind a project, and compiles them into the small amount of context an AI agent actually needs. Local first, git synced, no account.",
  openGraph: {
    title: "RecurOS — one memory for every AI tool you use",
    description:
      "Record a decision once. ChatGPT, claude.ai, Claude Code, Cursor and Codex all know it.",
    type: "website",
  },
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <header className="site-header">
          <div className="wrap">
            <Link className="brand" href="/">
              <Mark />
              RecurOS
            </Link>
            <nav className="nav">
              <Link href="/docs">Docs</Link>
              <Link href="/docs/numbers">Numbers</Link>
              <Link href="/releases">Releases</Link>
              <a href="https://github.com/Percobain/RecurOS">GitHub</a>
            </nav>
          </div>
        </header>
        <main>{children}</main>
        <footer className="wrap site-footer">
          <span>Apache-2.0</span>
          <a href="https://github.com/Percobain/RecurOS">Source</a>
          <Link href="/docs/numbers">How the numbers were measured</Link>
          <span className="sep">Your context stays on your machine.</span>
        </footer>
      </body>
    </html>
  );
}
