import { engineeringPosts as generatedEngineeringPosts } from "../generated-engineering";

export type EngineeringPost = (typeof generatedEngineeringPosts)[number];

export const engineeringPosts = generatedEngineeringPosts;

// Keep ids in sync with blogCategories in scripts/generate-content.mjs.
export const blogCategories = {
  engineering: {
    label: "Engineering",
    href: "/blogs",
  },
  incidents: {
    label: "Incidents",
    href: "/blogs/incidents",
    heading: "Incidents",
    eyebrow: "What went wrong",
    description: "Postmortems of incidents in Hubu: what happened, why, and what we are changing.",
  },
} as const;

export type BlogCategory = keyof typeof blogCategories;

export function postsInCategory(category: BlogCategory) {
  return engineeringPosts.filter((post) => post.category === category);
}

export function getEngineeringPost(slug: string) {
  return engineeringPosts.find((post) => post.slug === slug);
}

export function formatPublicationDate(date: string) {
  return new Intl.DateTimeFormat("en-US", {
    month: "long",
    day: "numeric",
    year: "numeric",
    timeZone: "UTC",
  }).format(new Date(`${date}T00:00:00.000Z`));
}
