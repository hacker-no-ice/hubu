import { HubuWordmark } from "./HubuWordmark";

export function EngineeringHeader({ index = false }: { index?: boolean }) {
  return (
    <header className="topbar engineering-header">
      <a className="brand" href="/" aria-label="Hubu home">
        <HubuWordmark className="brand-wordmark" decorative />
        <i>/ blogs</i>
      </a>
      <nav aria-label="Primary navigation">
        <a href="/blogs" aria-current={index ? "page" : undefined}>Engineering Blogs</a>
        <a href="/docs/overview">Documentation</a>
        <a href="https://github.com/hacker-no-ice/hubu">GitHub ↗</a>
      </nav>
    </header>
  );
}
