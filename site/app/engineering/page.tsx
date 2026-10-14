/* eslint-disable @next/next/no-img-element -- Editorial artwork is served as static assets with explicit dimensions. */
import { EngineeringHeader } from "../components/EngineeringHeader";
import { engineeringPosts, formatPublicationDate } from "../lib/engineering";
import { engineeringIndexMetadata } from "../lib/metadata";

export const metadata = engineeringIndexMetadata;

export default function EngineeringPage() {
  return (
    <div className="engineering-shell">
      <EngineeringHeader index />
      <main className="engineering-index" id="main-content">
        <header className="engineering-index-heading">
          <p className="eyebrow"><span /> How we build Hubu</p>
          <h1>Engineering</h1>
          <p>Design decisions, tradeoffs, and lessons from building governed spend for AI agents.</p>
        </header>
        <div className="engineering-posts">
          {engineeringPosts.map((post) => (
            <article className="engineering-card" key={post.slug}>
              <a href={post.href}>
                <img src={post.cover.src} alt={post.cover.alt} width={post.cover.width} height={post.cover.height} decoding="async" />
                <div className="engineering-card-copy">
                  <p className="engineering-published">Published <time dateTime={post.publishedOn}>{formatPublicationDate(post.publishedOn)}</time></p>
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
