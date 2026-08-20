import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { DocsShell } from "../../components/DocsShell";
import { getDocument } from "../../lib/docs";

type Props = { params: Promise<{ slug: string[] }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { slug } = await params;
  const document = getDocument(slug.join("/"));
  return document ? { title: document.title, description: document.excerpt } : {};
}

export default async function DocumentationPage({ params }: Props) {
  const { slug } = await params;
  const document = getDocument(slug.join("/"));
  if (!document) notFound();
  return <DocsShell document={document} />;
}
