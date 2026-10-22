import { HubuWordmark } from "./HubuWordmark";

export function EngineeringHeader({ index = false, section = "blogs" }: { index?: boolean; section?: "blogs" | "releases" }) {
  return (
    <header className="topbar engineering-header">
      <a className="brand" href="/" aria-label="Hubu home">
        <HubuWordmark className="brand-wordmark" decorative />
        <i>/ {section === "releases" ? "release notes" : "blogs"}</i>
      </a>
      <nav aria-label="Primary navigation">
        <a href="/blogs" aria-current={index ? "page" : undefined}>Engineering Blogs</a>
        <a href="/releases" aria-current={section === "releases" ? "page" : undefined}>Release notes</a>
        <a href="/docs/overview">Documentation</a>
        <a href="https://github.com/hacker-no-ice/hubu">GitHub ↗</a>
      </nav>
    </header>
  );
}
