import { documents } from "../generated-docs";

export type Doc = (typeof documents)[number];

export const navGroups = [
  { label: "Start here", items: [["Overview", "overview"], ["Send feedback", "feedback"], ["Local stack quick start", "local-stack"], ["Complete stack examples", "configuration/local-stack/v1/examples"]] },
  { label: "Configure the stack", items: [["Configuration reference", "configuration/local-stack/v1"], ["stack.toml", "configuration/local-stack/v1/stack-toml"], ["credentials.toml", "configuration/local-stack/v1/credentials-toml"], ["providers.toml", "configuration/local-stack/v1/providers-toml"], ["Decision guides", "configuration/local-stack/v1/decisions"]] },
  { label: "Core concepts", items: [["Agent registration", "agent-registration"], ["Policy engine", "policy-engine"], ["Spend lifecycle", "spend-lifecycle"], ["Gongbu execution", "gongbu-execution"], ["Unified MCP", "unified-mcp"]] },
  { label: "Operations & runbooks", items: [["CLI administration", "cli"], ["Local demo", "operations/local-demo"], ["Gongbu server", "operations/gongbu-server"], ["Gongbu sandbox", "operations/gongbu-sandbox"], ["Live provider operations", "operations/live-providers"], ["Gemini provider contract", "operations/gemini-provider-contract"], ["FLUX.2 provider contract", "operations/flux-provider-contract"], ["Benchmarking", "operations/benchmarking"], ["Releases", "operations/releases"], ["Repository security", "operations/repository-security"]] },
  { label: "Protocols & reference", items: [["Spend executor contract", "spend-executor-contract"]] },
] as const;

export const searchDocuments = documents.map(({ slug, href, title, excerpt }) => ({ slug, href, title, excerpt }));

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
