/* eslint-disable @next/next/no-img-element -- Static artwork is served directly by the site worker. */
import { EngineeringHeader } from "../components/EngineeringHeader";
import { documentMetadata } from "../lib/metadata";

export const metadata = documentMetadata({
  title: "How the mascot evolved", href: "/mascot",
  description: "How Hubu's seal mascot went from a concept sheet to governed variations to a design that matches the wordmark.",
});

const articleUrl = "https://x.com/hacker_no_ice/status/2118507555794374942";

type Image = { src: string; width: number; height: number; alt: string; caption: string; tone?: "light" | "evolved" };
type Stage = { title: string; copy: string; images: Image[]; link?: { href: string; label: string; external?: boolean } };

const stages: Stage[] = [
  {
    title: "A concept sheet",
    copy: "The mascot started as one of five concepts from a built-in image generator: a treasurer, a seal, a guardian, an owl, and an ink spirit. The seal was selected.",
    images: [{ src: "/demos/local-stack/concepts-builtin.png", width: 1536, height: 1024, alt: "Built-in image generator's five Hubu mascot concepts: treasurer, seal, guardian, owl, and ink spirit", caption: "Initial concept sheet, including the seal.", tone: "light" }],
  },
  {
    title: "The original seal",
    copy: "A cream-and-jade seal resting on a cinnabar base became Hubu's first mascot.",
    images: [{ src: "/brand/hubu-mascot.png", width: 1388, height: 1133, alt: "Original cream-and-jade Hubu seal mascot on a cinnabar base", caption: "The original mascot." }],
  },
  {
    title: "Governed variations",
    copy: "In the local stack demo, an agent iterated on the seal through Hubu. FLUX and Gemini produced variations, and policy decided which requests ran automatically and which needed a person.",
    images: [
      { src: "/demos/local-stack/mascot-flux-ledger.jpg", width: 1024, height: 1024, alt: "FLUX-generated Hubu mascot holding a green ledger", caption: "FLUX · holding a ledger · automatically allowed.", tone: "light" },
      { src: "/demos/local-stack/mascot-gemini-blocked.jpg", width: 1408, height: 768, alt: "Gemini-generated Hubu mascot with a stern expression, holding an open ledger", caption: "Gemini · a stern expression · approved by a person.", tone: "light" },
    ],
    link: { href: "/demos/local-stack#mascot-gallery", label: "Watch the agent create variations →" },
  },
  {
    title: "Evolved to match the wordmark",
    copy: "The seal then evolved to share the wordmark’s theme, with violet and cyan light along its jade swirls.",
    images: [{ src: "/brand/hubu-mascot-evolved.jpg", width: 1254, height: 975, alt: "Evolved Hubu seal mascot on a cinnabar base, with violet and cyan glowing edges", caption: "The current mascot.", tone: "evolved" }],
    link: { href: articleUrl, label: "Read how it evolved ↗", external: true },
  },
];

export default function MascotHistory() {
  return <div className="engineering-shell">
    <EngineeringHeader section="mascot" />
    <main className="engineering-article mascot-history" id="main-content">
      <article className="engineering-article-inner">
        <header className="mascot-history-heading">
          <a className="engineering-back" href="/">← Home</a>
          <p className="eyebrow"><span /> Hubu&apos;s mascot</p>
          <h1>How the mascot evolved.</h1>
          <p className="engineering-deck">From a concept sheet, through variations an agent made under Hubu&apos;s spending rules, to a design that matches the wordmark.</p>
        </header>
        <ol className="mascot-timeline">
          {stages.map((stage, index) => <li className="mascot-stage" key={stage.title}>
            <div className="mascot-stage-marker" aria-hidden="true"><span>{`0${index + 1}`}</span></div>
            <div>
              <h2>{stage.title}</h2>
              <p>{stage.copy}</p>
              <div className="mascot-stage-images">
                {stage.images.map((image) => <figure key={image.src} className={image.tone ? `mascot-${image.tone}` : undefined}>
                  <a href={image.src}><img src={image.src} width={image.width} height={image.height} alt={image.alt} loading="lazy" decoding="async" /></a>
                  <figcaption>{image.caption}</figcaption>
                </figure>)}
              </div>
              {stage.link && <a className="mascot-stage-link" href={stage.link.href} {...(stage.link.external ? { rel: "noopener" } : {})}>{stage.link.label}</a>}
            </div>
          </li>)}
        </ol>
      </article>
    </main>
  </div>;
}
