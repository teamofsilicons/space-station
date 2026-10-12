// Docs for anyone, signed in or not, at /docs/:slug — rendered once, at build.
import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { loadDocs, SLUGS } from "../../../docs-content";
import { Doc } from "../../../docs";
import { PublicBar } from "../../../public";

type Props = { params: Promise<{ slug?: string[] }> };
const pick = async ({ params }: Props) => {
  const { slug = [] } = await params;
  return slug.length > 1 ? undefined : slug[0] || "getting-started";
};

export const dynamicParams = false;
export const generateStaticParams = () => [{ slug: [] }, ...SLUGS.map((s) => ({ slug: [s] }))];
export async function generateMetadata(props: Props): Promise<Metadata> {
  const slug = await pick(props);
  return { title: `${loadDocs().find((d) => d.slug === slug)?.title || "Docs"} · Space Station` };
}

export default async function DocsRoute(props: Props) {
  const slug = await pick(props);
  if (!slug || !SLUGS.includes(slug)) notFound();
  return (
    <div className="standalone">
      <PublicBar />
      <main className="standalone-body">
        <Doc docs={loadDocs()} slug={slug} base="/docs/" />
      </main>
    </div>
  );
}
