import { engineeringPosts as generatedEngineeringPosts } from "../generated-engineering";

export type EngineeringPost = (typeof generatedEngineeringPosts)[number];

export const engineeringPosts = generatedEngineeringPosts;

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
