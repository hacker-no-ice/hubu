/* eslint-disable @next/next/no-img-element -- Editorial artwork is served as static assets with explicit dimensions. */
import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { EngineeringHeader } from "../../components/EngineeringHeader";
import { getEngineeringPost, formatPublicationDate } from "../../lib/engineering";
import { engineeringPostMetadata } from "../../lib/metadata";

type Props = { params: Promise<{ slug: string }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { slug } = await params;
  return engineeringPostMetadata(getEngineeringPost(slug));
}

export default async function EngineeringPostPage({ params }: Props) {
  const { slug } = await params;
  const post = getEngineeringPost(slug);
  if (!post) notFound();

  return (
    <div className="engineering-shell">
      <EngineeringHeader />
      <main className="engineering-article" id="main-content">
        <article className="engineering-article-inner">
          <header className="engineering-article-heading">
            <a className="engineering-back" href="/engineering">← Engineering</a>
            <h1>{post.title}</h1>
            <p className="engineering-deck">{post.description}</p>
            <div className="engineering-article-meta">
              <p>Published <time dateTime={post.publishedOn}>{formatPublicationDate(post.publishedOn)}</time></p>
              {post.releaseStatus ? <p className="engineering-release-status">{post.releaseStatus}</p> : null}
            </div>
          </header>
          <figure className="engineering-cover">
            <img src={post.cover.src} alt={post.cover.alt} width={post.cover.width} height={post.cover.height} fetchPriority="high" decoding="async" />
          </figure>
          <div className="engineering-prose" dangerouslySetInnerHTML={{ __html: post.html }} />
          <footer className="engineering-article-footer"><a href="/engineering">← All engineering posts</a></footer>
        </article>
      </main>
    </div>
  );
}
