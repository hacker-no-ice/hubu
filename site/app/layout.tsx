import type { Metadata } from "next";
import { headers } from "next/headers";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export async function generateMetadata(): Promise<Metadata> {
  const requestHeaders = await headers();
  const forwardedHost = requestHeaders.get("x-forwarded-host") ?? requestHeaders.get("host") ?? "localhost";
  const host = /^[a-z0-9.-]+(?::\d+)?$/i.test(forwardedHost) ? forwardedHost : "localhost";
  const protocol = requestHeaders.get("x-forwarded-proto") === "http" || host.startsWith("localhost") ? "http" : "https";
  const metadataBase = new URL(`${protocol}://${host}`);
  const image = new URL("/og-architecture.png", metadataBase).toString();

  return {
    metadataBase,
    title: { default: "Hubu Docs — Governed spend for AI agents", template: "%s · Hubu Docs" },
    description: "Documentation for Hubu's local-first agent spend control plane and the Gongbu execution plane.",
    openGraph: {
      title: "Hubu / 户部",
      description: "Architecture in service of trust · One request · Two owners",
      images: [{ url: image, width: 1200, height: 630 }],
      type: "website",
    },
    twitter: {
      card: "summary_large_image",
      title: "Hubu / 户部",
      description: "Architecture in service of trust · One request · Two owners",
      images: [image],
    },
  };
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body
        className={`${geistSans.variable} ${geistMono.variable} antialiased`}
      >
        <a className="skip-link" href="#main-content">Skip to content</a>
        {children}
      </body>
    </html>
  );
}
