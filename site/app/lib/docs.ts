import { documents } from "../generated-docs";

export type Doc = (typeof documents)[number];

export const navGroups = [
  { label: "Start here", items: [["Overview", "overview"], ["Local stack quick start", "local-stack"], ["Complete stack examples", "configuration/local-stack/v1/examples"], ["Send feedback", "feedback"]] },
  { label: "Configure the stack", items: [["Configuration reference", "configuration/local-stack/v1"], ["stack.toml", "configuration/local-stack/v1/stack-toml"], ["credentials.toml", "configuration/local-stack/v1/credentials-toml"], ["providers.toml", "configuration/local-stack/v1/providers-toml"], ["Decision guides", "configuration/local-stack/v1/decisions"]] },
  { label: "Core concepts", items: [["Agent registration", "agent-registration"], ["Policy engine", "policy-engine"], ["Spend lifecycle", "spend-lifecycle"], ["Gongbu execution", "gongbu-execution"], ["Unified MCP", "unified-mcp"], ["Use your own executor", "external-executor"], ["MCP tool catalog", "mcp-tool-consolidation"]] },
  { label: "Operations & runbooks", items: [["CLI administration", "cli"], ["Managing a local stack", "operations/managing-a-stack"], ["Ledger history", "ledger-history"], ["Local demo", "operations/local-demo"], ["Gongbu server (advanced)", "operations/gongbu-server"], ["Live provider operations", "operations/live-providers"], ["Gemini provider contract", "operations/gemini-provider-contract"], ["FLUX.2 provider contract", "operations/flux-provider-contract"], ["Releases", "operations/releases"]] },
  { label: "Protocols & reference", items: [["Spend executor contract", "spend-executor-contract"], ["Executor conformance", "executor-conformance"]] },
  { label: "Experimental integrations", items: [["ChatGPT MCP tunnel", "operations/chatgpt-mcp-tunnel"]] },
] as const;

export const searchDocuments = documents.map(({ slug, href, title, excerpt }) => ({ slug, href, title, excerpt }));

export function getDocument(slug: string) {
  return documents.find((doc) => doc.slug === slug);
}

// Pagination uses the curated navigation labels, which are shorter than some
// document titles (the overview's title is the "Hubu / 户部" brand line).
export function adjacentDocuments(slug: string) {
  const order: (readonly [string, string])[] = navGroups.flatMap((group): (readonly [string, string])[] => [...group.items]);
  const index = order.findIndex(([, itemSlug]) => itemSlug === slug);
  const link = (position: number) => {
    const [label, itemSlug] = order[position];
    const document = getDocument(itemSlug);
    return document ? { href: document.href, label } : undefined;
  };
  return {
    previous: index > 0 ? link(index - 1) : undefined,
    next: index >= 0 && index < order.length - 1 ? link(index + 1) : undefined,
  };
}
