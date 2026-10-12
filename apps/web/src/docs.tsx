"use client";
// The documentation, read in a tab beside the work or on its own public page. The server renders the
// markdown (docs-content); a link between documents stays where it is read.
import { createContext, useContext } from "react";
import type { DocEntry } from "./docs-content";
import { useTab, useTitle } from "./ui";

export const DocsContext = createContext<DocEntry[]>([]);
export const useDocs = () => useContext(DocsContext);

export function Doc(p: { docs: DocEntry[]; slug: string; base: string; go?: (slug: string) => void }) {
  const doc = p.docs.find((d) => d.slug === p.slug);
  const follow = (e: React.MouseEvent, slug: string) => {
    if (!p.go || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    e.preventDefault();
    p.go(slug);
  };
  return (
    <div className="doc">
      <nav className="doc-nav" aria-label="Documentation">
        <span className="kicker">Documentation</span>
        {p.docs.map((d) => (
          <a key={d.slug} href={p.base + d.slug} className={d.slug === p.slug ? "on" : undefined} onClick={(e) => follow(e, d.slug)}>
            {d.title}
          </a>
        ))}
      </nav>
      <article
        className="prose"
        dangerouslySetInnerHTML={{ __html: doc?.html || "<h1>Document not found</h1>" }}
        onClick={(e) => {
          const m = /^\/docs\/([a-z0-9-]+)/.exec((e.target as HTMLElement).closest("a")?.getAttribute("href") || "");
          if (m) follow(e, m[1]);
        }}
      />
    </div>
  );
}

/** Docs inside the workspace, as a tab. */
export function DocsPage() {
  const tab = useTab();
  const docs = useDocs();
  const slug = tab.route.id || "getting-started";
  const base = `/a/${tab.route.actor}/docs/`;
  useTitle(docs.find((d) => d.slug === slug)?.title || "Docs");
  return (
    <div className="page-body">
      <Doc docs={docs} slug={slug} base={base} go={(s) => tab.go(base + s)} />
    </div>
  );
}
