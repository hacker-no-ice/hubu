/* eslint-disable @next/next/no-img-element -- Editorial artwork is served as static assets with explicit dimensions. */
import { EngineeringHeader } from "./EngineeringHeader";
import { blogCategories, formatPublicationDate, type EngineeringPost } from "../lib/engineering";

type Props = {
  eyebrow: string;
  heading: string;
  description: string;
  posts: readonly EngineeringPost[];
  current: "all" | "incidents";
};

export function BlogIndex({ eyebrow, heading, description, posts, current }: Props) {
  return (
    <div className="engineering-shell">
      <EngineeringHeader index={current === "all"} />
      <main className="engineering-index" id="main-content">
        <header className="engineering-index-heading">
          <p className="eyebrow"><span /> {eyebrow}</p>
          <h1>{heading}</h1>
          <p>{description}</p>
        </header>
        <nav className="engineering-categories" aria-label="Blog categories">
          <a href="/blogs" aria-current={current === "all" ? "page" : undefined}>All posts</a>
          <a href={blogCategories.incidents.href} aria-current={current === "incidents" ? "page" : undefined}>{blogCategories.incidents.label}</a>
        </nav>
        <div className="engineering-posts">
          {posts.map((post) => (
            <article className="engineering-card" key={post.slug}>
              <a href={post.href}>
                <img src={post.cover.src} alt={post.cover.alt} width={post.cover.width} height={post.cover.height} decoding="async" />
                <div className="engineering-card-copy">
                  <p className="engineering-published">
                    <span className="engineering-category" data-category={post.category}>{blogCategories[post.category].label}</span>
                    {" · "}Published <time dateTime={post.publishedOn}>{formatPublicationDate(post.publishedOn)}</time>
                  </p>
                  <h2>{post.title}</h2>
                  <p>{post.description}</p>
                  <span className="engineering-read">Read the post <span aria-hidden="true">↗</span></span>
                </div>
              </a>
            </article>
          ))}
        </div>
      </main>
    </div>
  );
}
