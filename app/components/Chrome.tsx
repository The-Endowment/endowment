import Link from "next/link";
import { SolidLogo } from "@/components/Logo";
import { links } from "@/lib/site";

export function SiteHeader() {
  return (
    <header className="site-header">
      <div className="wrap header-inner">
        <Link href="/" className="brand">
          <SolidLogo className="brand-mark" />
          <span>The $PENIS Endowment</span>
        </Link>
        <nav aria-label="Primary" className="nav">
          <Link href="/#how">How it works</Link>
          <Link href="/thesis">Thesis</Link>
          <Link href="/#coin">The coin</Link>
          <Link href="/#faq">FAQ</Link>
          <Link href="/#delegate" className="button button-primary button-small">
            Delegate PUMP
          </Link>
        </nav>
      </div>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="site-footer">
      <div className="wrap footer-inner">
        <span>Not financial advice. Unaudited while in beta. Independent of the coin&rsquo;s creators.</span>
        <span className="footer-links">
          <a href={links.x}>@PenisEndowment</a>
          <a href={links.github}>GitHub</a>
          <a href={links.stonkfun}>$PENIS on stonk.fun</a>
        </span>
      </div>
    </footer>
  );
}
