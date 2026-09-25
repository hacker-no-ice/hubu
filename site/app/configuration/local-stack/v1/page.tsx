import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { DocsShell } from "../../../components/DocsShell";
import { getDocument } from "../../../lib/docs";
import { documentMetadata } from "../../../lib/metadata";

const documentSlug = "configuration/local-stack/v1";

export function generateMetadata(): Metadata {
  const document = getDocument(documentSlug);
  return documentMetadata(document);
}

export default function LocalStackConfigurationReference() {
  const document = getDocument(documentSlug);
  if (!document) notFound();
  return <DocsShell document={document} />;
}
