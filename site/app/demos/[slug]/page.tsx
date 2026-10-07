import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { DocsShell } from "../../components/DocsShell";
import { getDocument } from "../../lib/docs";
import { documentMetadata } from "../../lib/metadata";

type Props = { params: Promise<{ slug: string }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { slug } = await params;
  return documentMetadata(getDocument(`demos/${slug}`));
}

export default async function DemoPage({ params }: Props) {
  const { slug } = await params;
  const document = getDocument(`demos/${slug}`);
  if (!document) notFound();
  return <DocsShell document={document} />;
}
