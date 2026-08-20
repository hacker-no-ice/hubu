import { documents } from "../generated-docs";

export type Doc = (typeof documents)[number];

export const navGroups = [
  { label: "Start here", items: [["Overview", "overview"], ["Local stack quick start", "local-stack"]] },
  { label: "Core concepts", items: [["Agent registration", "agent-registration"], ["Policy engine", "policy-engine"], ["Spend lifecycle", "spend-lifecycle"], ["Gongbu execution", "gongbu-execution"], ["Unified MCP", "unified-mcp"]] },
  { label: "Operations & runbooks", items: [["Local demo", "operations/local-demo"], ["Gongbu server", "operations/gongbu-server"], ["Gongbu sandbox", "operations/gongbu-sandbox"], ["Live provider testing", "operations/live-provider-testing"], ["Benchmarking", "operations/benchmarking"], ["Releases", "operations/releases"], ["Repository security", "operations/repository-security"]] },
  { label: "Protocols & reference", items: [["Spend executor contract", "spend-executor-contract"]] },
] as const;

export const searchDocuments = documents.map(({ slug, title, excerpt }) => ({ slug, title, excerpt }));

export function getDocument(slug: string) {
  return documents.find((doc) => doc.slug === slug);
}

export function adjacentDocuments(slug: string) {
  const order = navGroups.flatMap((group) => group.items.map(([, itemSlug]) => itemSlug));
  const index = order.indexOf(slug as never);
  return {
    previous: index > 0 ? getDocument(order[index - 1]) : undefined,
    next: index >= 0 && index < order.length - 1 ? getDocument(order[index + 1]) : undefined,
  };
}
