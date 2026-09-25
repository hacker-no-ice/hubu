import type { Metadata } from "next";

export const siteName = "Hubu Docs";
export const siteOrigin = "https://hubustack.dev";
const defaultTitle = "Hubu Docs — Governed spend for AI agents";
const defaultDescription = "Documentation for Hubu's local-first agent spend control plane and the Gongbu execution plane.";
const socialImage = { url: `${siteOrigin}/og-wordmark.png`, width: 1200, height: 630 };

// Child metadata replaces the parent's openGraph and twitter objects wholesale,
// so each page builds complete share metadata rather than inheriting parts.
function shareMetadata(title: string, description: string, path?: string): Metadata {
  return {
    description,
    openGraph: { title, description, ...(path ? { url: path } : {}), siteName, type: "website", images: [socialImage] },
    twitter: { card: "summary_large_image", title, description, images: [socialImage.url] },
  };
}

export const siteMetadata: Metadata = {
  metadataBase: new URL(siteOrigin),
  title: { default: defaultTitle, template: `%s · ${siteName}` },
  icons: { icon: { url: "/favicon.svg", type: "image/svg+xml" } },
  ...shareMetadata(defaultTitle, defaultDescription),
};

export const homeMetadata: Metadata = {
  alternates: { canonical: "/" },
  ...shareMetadata(defaultTitle, defaultDescription, "/"),
};

export function documentMetadata(document: { title: string; description: string; href: string } | undefined): Metadata {
  if (!document) return {};
  return {
    title: document.title,
    alternates: { canonical: document.href },
    ...shareMetadata(`${document.title} · ${siteName}`, document.description, document.href),
  };
}
