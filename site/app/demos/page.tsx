import { DocsShell } from "../components/DocsShell";
import { getDocument } from "../lib/docs";
import { documentMetadata } from "../lib/metadata";

const document = getDocument("demos")!;
export const metadata = documentMetadata(document);

export default function DemosPage() {
  return <DocsShell document={document} />;
}
