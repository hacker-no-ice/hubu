import { BlogIndex } from "../../components/BlogIndex";
import { blogCategories, postsInCategory } from "../../lib/engineering";
import { incidentsIndexMetadata } from "../../lib/metadata";

export const metadata = incidentsIndexMetadata;

export default function IncidentsPage() {
  const category = blogCategories.incidents;
  return (
    <BlogIndex
      current="incidents"
      eyebrow={category.eyebrow}
      heading={category.heading}
      description={category.description}
      posts={postsInCategory("incidents")}
    />
  );
}
