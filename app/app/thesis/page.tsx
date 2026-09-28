import type { Metadata } from "next";
import Link from "next/link";

export const metadata: Metadata = {
  title: "The landlord thesis",
  description: "Why the biggest holders of a dividend-paying coin stay, and why the endowment exists.",
};

const risks = [
  { title: "The dividend is the only glue", body: "If volume dies, the reason to hold goes with it." },
  { title: "Landlords can still sell", body: "Nothing locks them in. It's a choice they make every day." },
  { title: "Trust can't be verified", body: "A promise to hold is reputational, not written on-chain." },
  { title: "A thin float cuts both ways", body: "Upside is violent, and so is a big holder's exit." },
];

export default function Thesis() {
  return (
    <>
      <section className="wrap" style={{ paddingTop: "clamp(72px, 9vw, 140px)", paddingBottom: 96, borderBottom: "1px solid var(--rule)" }}>
        <div style={{ display: "flex", flexDirection: "column", gap: 28 }}>
          <div className="eyebrow">The landlord thesis</div>
          <h1 className="display h1" style={{ maxWidth: 1100 }}>
            Why the biggest holders <em className="display-em">stay.</em>
          </h1>
          <p className="lede" style={{ maxWidth: 760 }}>
            On most meme coins, a large holder is a threat. On a coin that pays dividends, they can be an anchor. This
            is the idea the endowment is built on.
          </p>
        </div>
      </section>

      <section className="wrap section thesis" style={{ paddingTop: 96 }}>
        <nav className="toc" aria-label="In this piece">
          <span className="eyebrow" style={{ color: "var(--stone)", letterSpacing: 1 }}>
            In this piece
          </span>
          <a href="#old-problem">01 · The old problem</a>
          <a href="#dividends">02 · What dividends change</a>
          <a href="#landlord">03 · The landlord</a>
          <a href="#volume">04 · A bet on volume</a>
          <a href="#breaks">05 · Where it breaks</a>
          <a href="#endowment">06 · Why an endowment</a>
        </nav>

        <div style={{ display: "flex", flexDirection: "column", gap: 72 }}>
          <article id="old-problem" className="article">
            <div className="step-num">01</div>
            <h2 className="display h3">The old problem</h2>
            <p>
              On a normal meme coin, holding pays nothing. The only way a big holder gets paid is by selling, so the
              market treats every large wallet as future sell pressure. That&rsquo;s why &ldquo;top holder
              percentage&rdquo; became a safety check.
            </p>
          </article>

          <article id="dividends" className="article">
            <div className="step-num">02</div>
            <h2 className="display h3">What dividends change</h2>
            <p>
              $PENIS pays its holders a share of every trade, in PUMP. That turns a large position into an
              income-producing asset: the bigger the stake, the bigger the payout. Selling doesn&rsquo;t just hurt the
              chart. It gives up the income.
            </p>
          </article>

          <article id="landlord" className="article">
            <div className="step-num">03</div>
            <h2 className="display h3">The landlord</h2>
            <p>
              A landlord accumulates supply because holding is the profitable move. They take coins off the market and
              collect rent from the traders who create volume. Traders get a market where most of the supply is parked
              with people paid not to sell.
            </p>
            <blockquote className="quote">Trading creates rent. Rent keeps supply off the market.</blockquote>
          </article>

          <article id="volume" className="article">
            <div className="step-num">04</div>
            <h2 className="display h3">A bet on volume, not price</h2>
            <p>
              Dividends come from trading activity, so a landlord is really betting that volume lasts. A good landlord
              judges a coin the way a property investor judges a neighborhood: not what it rents for today, but what it
              will rent for over the next few years.
            </p>
          </article>

          <article id="breaks" className="article">
            <div className="step-num">05</div>
            <h2 className="display h3">Where it breaks</h2>
            <div className="grid-2" style={{ gap: 16 }}>
              {risks.map((r) => (
                <div key={r.title} className="card" style={{ padding: 24 }}>
                  <div style={{ fontSize: 17, fontWeight: 500, marginBottom: 8 }}>{r.title}</div>
                  <div className="muted" style={{ fontSize: 16 }}>
                    {r.body}
                  </div>
                </div>
              ))}
            </div>
          </article>

          <article id="endowment" className="article">
            <div className="step-num">06</div>
            <h2 className="display h3">Why an endowment</h2>
            <p>
              The endowment fixes the weakest link: it turns a promise into code. It&rsquo;s a landlord that can never
              sell, funded by landlords who choose to reinvest their rent. Its commitment isn&rsquo;t reputational.
              It&rsquo;s structural.
            </p>
          </article>
        </div>
      </section>

      <section className="wrap section">
        <div className="card cta" style={{ padding: "clamp(28px, 5vw, 72px)" }}>
          <div style={{ display: "flex", flexDirection: "column", gap: 14, maxWidth: 720 }}>
            <div className="display" style={{ fontSize: "clamp(30px, 3.4vw, 44px)", lineHeight: 1.1 }}>
              Concentration used to be pure overhang. Now it can be an anchor.
            </div>
            <div className="muted">From the original landlord thesis by @yourfriendbrett.</div>
          </div>
          <Link href="/#delegate" className="button button-primary">
            Delegate your PUMP
          </Link>
        </div>
      </section>
    </>
  );
}
