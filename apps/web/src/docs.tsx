// The documentation: docs/*.md bundled at build time, read in a tab beside the work or on its own
// public page. A link between documents stays where it is read. Also the public inspirations page.
import { createEffect, createMemo, createSignal, For, onCleanup } from "solid-js";
import { marked } from "marked";
import { useTab } from "./ui";
import { Icon, Mark } from "./icons";

const documents = import.meta.glob("../docs/*.md", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const bySlug = Object.fromEntries(Object.entries(documents).map(([path, md]) => [path.split("/").pop()!.replace(/\.md$/, ""), md]));
export const DOCS = ["getting-started", "web", "space-windows", "sql", "notifications", "rust", "cli", "credentials", "api"].map((slug) => ({
  slug,
  title: /^#\s+(.+)$/m.exec(bySlug[slug] || "")?.[1] || slug,
}));

function Doc(p: { slug: string; href: (slug: string) => string; go: (slug: string) => void }) {
  const html = createMemo(() => marked.parse(bySlug[p.slug] || "# Document not found", { async: false }) as string);
  return (
    <div class="doc">
      <nav class="doc-nav" aria-label="Documentation">
        <span class="kicker">Documentation</span>
        <For each={DOCS}>
          {(d) => (
            <a
              href={p.href(d.slug)}
              classList={{ on: d.slug === p.slug }}
              onClick={(e) => {
                if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
                e.preventDefault();
                p.go(d.slug);
              }}
            >
              {d.title}
            </a>
          )}
        </For>
      </nav>
      <article
        class="prose"
        innerHTML={html()}
        onClick={(e) => {
          const a = (e.target as HTMLElement).closest("a");
          const m = a && /^\/docs\/([a-z0-9-]+)/.exec(a.getAttribute("href") || "");
          if (!m || e.metaKey || e.ctrlKey || e.shiftKey) return;
          e.preventDefault();
          p.go(m[1]);
        }}
      />
    </div>
  );
}

/** Docs inside the workspace, as a tab. */
export function DocsPage() {
  const tab = useTab();
  const slug = () => tab.route().id || "getting-started";
  const base = () => `/o/${tab.route().org}/docs/`;
  tab.title(DOCS.find((d) => d.slug === slug())?.title || "Docs");
  return (
    <div class="page-body">
      <Doc slug={slug()} href={(s) => base() + s} go={(s) => tab.go(base() + s)} />
    </div>
  );
}

/** Docs for anyone, signed in or not, at /docs/:slug. */
export function PublicDocs() {
  const read = () => location.pathname.split("/")[2] || "getting-started";
  const [slug, setSlug] = createSignal(read());
  const pop = () => setSlug(read());
  addEventListener("popstate", pop);
  onCleanup(() => removeEventListener("popstate", pop));
  createEffect(() => (document.title = `${DOCS.find((d) => d.slug === slug())?.title || "Docs"} · Space Station`));
  return (
    <div class="standalone">
      <PublicBar />
      <main class="standalone-body">
        <Doc
          slug={slug()}
          href={(s) => `/docs/${s}`}
          go={(s) => {
            history.pushState(null, "", `/docs/${s}`);
            setSlug(s);
            scrollTo(0, 0);
          }}
        />
      </main>
    </div>
  );
}

export function PublicBar() {
  return (
    <header class="public-bar">
      <a class="brand" href="/">
        <Mark size={20} />
        Space Station
      </a>
      <nav>
        <a href="/docs">Docs</a>
        <a href="/inspirations">Inspirations</a>
      </nav>
      <a class="button primary small" href="/">
        Open the station <Icon name="forward" />
      </a>
    </header>
  );
}

export function Inspirations() {
  const cards = [
    ["See the whole orbit", "Heart Aerospace", "SPACE / CLARITY", "A single calm frame can carry a complex system. We borrow the confidence: one strong surface, one next move."],
    ["Explain by unfolding", "Postevand", "FLOW / CONTEXT", "Product stories feel clearer when each chapter earns its place. Space Station turns ingest, shape, and notify into a visible route."],
    ["Make progress legible", "X Business", "SEQUENCE / MOMENTUM", "A numbered path reduces hesitation. Every table, window, and notification should feel like the next instrument on a flight deck."],
    ["Let the grid breathe", "Cosmos", "COLLECT / CONTRAST", "Small marks become a field. Dither, orbit lines, and quiet hover states give the interface a sense of depth without adding noise."],
    ["Use the edge as a cue", "Arc", "SURFACE / FOCUS", "The sidebar belongs to the room; the paper belongs to the work. A little separation makes the central canvas feel held."],
    ["Tell the truth plainly", "Making Software", "WORDS / WEIGHT", "The best line is often the one that says exactly what happened: records arrived, a view changed, a message went out."],
  ] as const;
  document.title = "Inspirations · Space Station";
  return (
    <div class="standalone">
      <PublicBar />
      <main class="standalone-body inspirations">
        <span class="kicker">Field notes / 06</span>
        <h1>Inspirations for the station</h1>
        <p class="lede">A small atlas of the ideas shaping Space Station: systems that feel capable, calm, and a little unexpected.</p>
        <div class="card-grid">
          <For each={cards}>
            {(card) => (
              <section class="card">
                <span class="kicker accent">{card[2]}</span>
                <strong>{card[0]}</strong>
                <p class="muted">{card[3]}</p>
                <small>{card[1]}</small>
              </section>
            )}
          </For>
        </div>
      </main>
    </div>
  );
}

