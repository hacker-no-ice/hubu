import Link from "next/link";

export default function NotFound() {
  return <main className="not-found" id="main-content"><span className="brand-seal">户</span><p>404 · DOCUMENT NOT FOUND</p><h1>This page is outside the ledger.</h1><Link className="button primary" href="/docs/overview">Return to the docs →</Link></main>;
}
