// The documentation as the server reads it: docs/*.md rendered once, at build, in reading order.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { marked } from "marked";

export type DocEntry = { slug: string; title: string; html: string };
export const SLUGS = ["getting-started", "web", "space-windows", "sql", "notifications", "rust", "cli", "credentials", "api"];

let docs: DocEntry[] | undefined;
export function loadDocs(): DocEntry[] {
  return (docs ??= SLUGS.map((slug) => {
    const md = readFileSync(join(process.cwd(), "docs", `${slug}.md`), "utf8");
    return { slug, title: /^#\s+(.+)$/m.exec(md)?.[1] || slug, html: marked.parse(md, { async: false }) as string };
  }));
}
