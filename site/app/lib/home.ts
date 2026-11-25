import { documents } from "../generated-docs";
import { blogCategories, engineeringPosts, type BlogCategory } from "./engineering";
import { navGroups } from "./docs";

export type NavLink = { title: string; href: string; description?: string; meta?: { label: string; tone?: string; date?: string; dateLabel?: string } };
export type NavColumn = { heading: string; links: NavLink[]; layout?: "list" | "rows" };
export type NavFeature = { kicker: string; title: string; description: string; href: string; cta: string; code?: string };
export type NavPanel = { id: string; label: string; columns: NavColumn[]; feature: NavFeature };

const changelog = "https://github.com/hacker-no-ice/hubu/blob/main/CHANGELOG.md";

// The five most recent releases, newest first. Releases older than the site's
// release pages link to their CHANGELOG section. Update when a release ships.
export const recentReleases: NavLink[] = [
  { title: "Owner confirmation on your Mac; separate agent and owner credentials", href: "/releases/v0.3.0", meta: { label: "v0.3.0", tone: "latest", date: "2026-11-25", dateLabel: "Nov 25, 2026" } },
  { title: "Every provider runs from a verified provider pack", href: "/releases/v0.2.5", meta: { label: "v0.2.5", date: "2026-11-06", dateLabel: "Nov 6, 2026" } },
  { title: "Append-only governance evidence for policy and approvals", href: "/releases/v0.2.4", meta: { label: "v0.2.4", date: "2026-10-19", dateLabel: "Oct 19, 2026" } },
  { title: "hubu watch: a live terminal view of agent budgets", href: `${changelog}#v023--2026-10-13`, meta: { label: "v0.2.3", date: "2026-10-13", dateLabel: "Oct 13, 2026" } },
  { title: "Authorization records replace spend history", href: `${changelog}#v022--2026-10-05`, meta: { label: "v0.2.2", date: "2026-10-05", dateLabel: "Oct 5, 2026" } },
];

const shortDate = (date: string) => new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric", year: "numeric", timeZone: "UTC" }).format(new Date(`${date}T00:00:00.000Z`));

const latestPosts: NavLink[] = engineeringPosts.slice(0, 5).map((post) => ({
  title: post.title,
  href: post.href,
  meta: { label: blogCategories[post.category as BlogCategory].label, tone: post.category, date: post.publishedOn, dateLabel: shortDate(post.publishedOn) },
}));

const postCount = (category: BlogCategory) => engineeringPosts.filter((post) => post.category === category).length;
const plural = (count: number) => `${count} post${count === 1 ? "" : "s"}`;

export const navPanels: NavPanel[] = [
  {
    id: "docs",
    label: "Docs",
    columns: [
      { heading: "Start here", links: [
        { title: "Overview", href: "/docs/overview", description: "What Hubu and Gongbu do" },
        { title: "Why Hubu", href: "/docs/why-hubu", description: "Compared with building it yourself" },
        { title: "Local stack quick start", href: "/docs/local-stack", description: "Install to first governed request" },
        { title: "FAQ", href: "/docs/faq" },
        { title: "Send feedback", href: "/docs/feedback" },
      ] },
      { heading: "Core concepts", links: [
        { title: "Policy engine", href: "/docs/policy-engine", description: "Rules every spend is checked against" },
        { title: "Spend lifecycle", href: "/docs/spend-lifecycle", description: "Reserve, settle, release, reconcile" },
        { title: "Unified MCP", href: "/docs/unified-mcp", description: "The one agent-facing surface" },
        { title: "Agent registration", href: "/docs/agent-registration" },
        { title: "Threat model", href: "/docs/threat-model" },
      ] },
      { heading: "Operate", links: [
        { title: "Admin web UI", href: "/docs/admin-ui", description: "Approvals, policies, budgets" },
        { title: "CLI administration", href: "/docs/cli" },
        { title: "Configuration reference", href: "/docs/configuration/local-stack/v1" },
        { title: "Live provider operations", href: "/docs/operations/live-providers" },
        { title: "Provider pack schema", href: "/docs/provider-packs" },
      ] },
    ],
    feature: { kicker: "Quick start", title: "From install to a running stack", description: "Sandbox first, no provider keys needed.", href: "/docs/local-stack", cta: "Open the guide →", code: "hubu stack start" },
  },
  {
    id: "demos",
    label: "Demos",
    columns: [
      { heading: "Watch", links: [
        { title: "All demos", href: "/demos", description: "Recordings with companion scripts" },
        { title: "Sandbox: registration to settlement", href: "/demos/sandbox", description: "No provider credentials" },
        { title: "Local stack: real providers", href: "/demos/local-stack", description: "Live Gemini and FLUX calls" },
      ] },
      { heading: "Try", links: [
        { title: "Local demo runbook", href: "/docs/operations/local-demo", description: "Run the demo yourself" },
        { title: "Complete stack examples", href: "/docs/configuration/local-stack/v1/examples" },
        { title: "Meet Marty", href: "/mascot", description: "How the mascot evolved" },
      ] },
    ],
    feature: { kicker: "3 minutes", title: "Meet Hubu", description: "The introduction: agents get room to work, inside limits you set.", href: "/demos", cta: "Watch →" },
  },
  {
    id: "architecture",
    label: "Architecture",
    columns: [
      { heading: "Explore", links: [
        { title: "Architecture guide", href: "/architecture/", description: "Reader-focused tour of the system" },
        { title: "Engineering explorer", href: "/architecture/internal/", description: "Drill-down components with code links" },
      ] },
      { heading: "Boundaries", links: [
        { title: "Gongbu execution", href: "/docs/gongbu-execution", description: "The default executor" },
        { title: "Spend executor contract", href: "/docs/spend-executor-contract", description: "The versioned boundary" },
        { title: "Use your own executor", href: "/docs/external-executor" },
        { title: "Executor conformance", href: "/docs/executor-conformance" },
      ] },
    ],
    feature: { kicker: "Interactive", title: "Control plane + executors", description: "Follow one request from the agent through Hubu, the contract, and Gongbu.", href: "/architecture/", cta: "Explore →" },
  },
  {
    id: "blog",
    label: "Blog",
    columns: [
      { heading: "Latest posts", links: latestPosts, layout: "rows" },
      { heading: "Browse by category", links: (Object.keys(blogCategories) as BlogCategory[]).map((category) => ({ title: blogCategories[category].label, href: blogCategories[category].href, description: plural(postCount(category)) })) },
    ],
    feature: { kicker: "All posts", title: "Engineering Blogs", description: "How Hubu is built, and what broke along the way.", href: "/blogs", cta: "See all posts →" },
  },
  {
    id: "releases",
    label: "Releases",
    columns: [{ heading: "Recent releases", links: recentReleases, layout: "rows" }],
    feature: { kicker: "All releases", title: "Release notes", description: "Every release with its notes, system card, and verification evidence.", href: "/releases", cta: "See all releases →" },
  },
];

export type SearchEntry = { title: string; group: string; href: string; excerpt: string; headings: string[] };

const groupFor = new Map<string, string>(navGroups.flatMap((group) => group.items.map(([, slug]) => [slug as string, group.label as string])));

export const headerSearchIndex: SearchEntry[] = [
  ...documents.map((doc) => ({
    title: doc.title,
    group: groupFor.get(doc.slug) ?? "Docs",
    href: doc.href,
    excerpt: doc.excerpt,
    headings: doc.headings.map((heading) => heading.text).slice(0, 14),
  })),
  ...engineeringPosts.map((post) => ({ title: post.title, group: "Engineering blog", href: post.href, excerpt: post.description, headings: [] })),
  { title: "Architecture guide", group: "Architecture", href: "/architecture/", excerpt: "Interactive tour of Hubu, the executor contract, and Gongbu.", headings: ["control plane", "executors"] },
  { title: "Release notes", group: "Releases", href: "/releases", excerpt: "Release notes, system cards, and verification evidence for each Hubu release.", headings: recentReleases.map((release) => release.meta?.label ?? "") },
];

export const popularSearches = ["/docs/local-stack", "/docs/policy-engine", "/docs/unified-mcp", "/docs/admin-ui", "/demos"];
