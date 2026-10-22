import { HubuWordmark } from "./HubuWordmark";

export function EngineeringHeader({ index = false, section = "blogs" }: { index?: boolean; section?: "blogs" | "break-hubu" }) {
  return (
    <header className="topbar engineering-header">
      <a className="brand" href="/" aria-label="Hubu home">
        <HubuWordmark className="brand-wordmark" decorative />
        <i>/ {section === "break-hubu" ? "break hubu" : "blogs"}</i>
      </a>
      <nav aria-label="Primary navigation">
        <a href="/blogs" aria-current={index ? "page" : undefined}>Engineering Blogs</a>
        <a href="/break-hubu" aria-current={section === "break-hubu" ? "page" : undefined}>Break Hubu</a>
        <a href="/docs/overview">Documentation</a>
        <a href="https://github.com/hacker-no-ice/hubu">GitHub ↗</a>
      </nav>
    </header>
  );
}
