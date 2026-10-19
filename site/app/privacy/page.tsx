import type { Metadata } from "next";
import { AnalyticsPreference } from "./preference";

export const metadata: Metadata = {
  title: "Privacy & analytics · Hubu",
  description: "How hubustack.dev measures website visits and how to opt out.",
  alternates: { canonical: "https://hubustack.dev/privacy" },
};

export default function PrivacyPage() {
  return (
    <main id="main-content" className="section-wrap markdown-body">
      <a href="/">← Hubu home</a>
      <h1>Privacy &amp; analytics</h1>
      <p>We measure use of hubustack.dev to understand which documentation and articles are useful.</p>
      <h2>What we measure</h2>
      <p>When first-party analytics is enabled, a random first-party cookie identifies this browser for up to one year. We store a hash of that identifier, page paths, visit identifiers, timestamps, country, and the hostname of an external referring website in our Cloudflare D1 database. We do not store IP addresses, user-agent strings, URL query strings, or URL fragments in this analytics database.</p>
      <p>Page-view records and inactive visitor identifiers expire after 90 days. An active browser can retain its first-seen date longer. Unique visitors means estimated unique browsers, rather than identified people: different devices and cleared cookies count separately.</p>
      <h2>Your choice</h2>
      <p>The tracker honors Do Not Track and Global Privacy Control. You can also exclude this browser below. This preference applies to this browser on hubustack.dev and stops future first-party analytics events.</p>
      <AnalyticsPreference />
      <h2>Other services</h2>
      <p>Cloudflare also provides hosting, security, and its existing Web Analytics service. The choice above controls the additional first-party visitor dashboard. Cloudflare&apos;s services have their own data handling described in <a href="https://www.cloudflare.com/privacypolicy/">Cloudflare&apos;s privacy policy</a>.</p>
    </main>
  );
}
