import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { DocsShell } from "../../../../components/DocsShell";
import { getDocument } from "../../../../lib/docs";

type Props = { params: Promise<{ page: string }> };

function documentSlug(page: string) {
  return `configuration/local-stack/v1/${page}`;
}

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { page } = await params;
  const document = getDocument(documentSlug(page));
  return document ? { title: document.title, description: document.excerpt } : {};
}

export default async function LocalStackConfigurationPage({ params }: Props) {
  const { page } = await params;
  const document = getDocument(documentSlug(page));
  if (!document) notFound();
  return <DocsShell document={document} />;
}
