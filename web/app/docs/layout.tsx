import Link from "next/link";

const PAGES = [
  { href: "/docs", label: "Getting started" },
  { href: "/docs/existing-project", label: "Existing projects" },
  { href: "/docs/chats", label: "ChatGPT & claude.ai" },
  { href: "/docs/how-it-works", label: "How it works" },
  { href: "/docs/decisions", label: "What we used, and why" },
  { href: "/docs/numbers", label: "Numbers" },
  { href: "/docs/commands", label: "Command reference" },
];

export default function DocsLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="wrap docs">
      <aside className="sidebar">
        <strong>Documentation</strong>
        {PAGES.map((p) => (
          <Link key={p.href} href={p.href}>
            {p.label}
          </Link>
        ))}
      </aside>
      <article className="doc">{children}</article>
    </div>
  );
}
