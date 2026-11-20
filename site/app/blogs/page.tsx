import { BlogIndex } from "../components/BlogIndex";
import { engineeringPosts } from "../lib/engineering";
import { engineeringIndexMetadata } from "../lib/metadata";

export const metadata = engineeringIndexMetadata;

export default function EngineeringPage() {
  return (
    <BlogIndex
      current="all"
      eyebrow="How we build Hubu"
      heading="Engineering Blogs"
      description="Design decisions, tradeoffs, and lessons from building governed spend for AI agents."
      posts={engineeringPosts}
    />
  );
}
