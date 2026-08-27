import type { Metadata } from "next";
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

const metadataBase = new URL("https://hubustack.dev");
const socialImage = new URL("/og-wordmark.png", metadataBase).toString();

export const metadata: Metadata = {
  metadataBase,
  title: { default: "Hubu Docs — Governed spend for AI agents", template: "%s · Hubu Docs" },
  description: "Documentation for Hubu's local-first agent spend control plane and the Gongbu execution plane.",
  openGraph: {
    title: "Hubu / 户部",
    description: "Architecture in service of trust · Hubu governs · Gongbu executes",
    images: [{ url: socialImage, width: 1200, height: 630 }],
    type: "website",
  },
  twitter: {
    card: "summary_large_image",
    title: "Hubu / 户部",
    description: "Architecture in service of trust · Hubu governs · Gongbu executes",
    images: [socialImage],
  },
};

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
