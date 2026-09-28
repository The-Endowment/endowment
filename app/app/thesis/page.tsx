import type { Metadata } from "next";
import { links } from "@/lib/site";

export const metadata: Metadata = {
  title: "The landlord thesis",
  description: "Why the biggest holders of a dividend-paying coin stay, and why the endowment exists.",
};

export default function Thesis() {
  return (
    <>
      <section className="wrap">
        <div className="thesis-head">
          <h1 className="display h1">The landlord thesis</h1>
          <p className="lede">
            On most meme coins, a large holder is a threat. On a coin that pays dividends, they can be an anchor. This
            is the idea the endowment is built on.
          </p>
        </div>
      </section>

      <div className="wrap">
        <section className="row">
          <h2 className="row-label">The old problem</h2>
          <div className="prose">
            <p>
              On a normal meme coin, holding pays nothing. The only way a big holder gets paid is by selling, so the
              market treats every large wallet as future sell pressure. That&rsquo;s why &ldquo;top holder
              percentage&rdquo; became a safety check.
            </p>
          </div>
        </section>

        <section className="row">
          <h2 className="row-label">What dividends change</h2>
          <div className="prose">
            <p>
              $PENIS pays its holders a share of every trade, in PUMP. That turns a large position into an asset that
              produces income, and the bigger the stake, the bigger the payout. Selling no longer just hurts the
              chart. It gives up the income.
            </p>
          </div>
        </section>

        <section className="row">
          <h2 className="row-label">The landlord</h2>
          <div className="prose">
            <p>
              A landlord accumulates supply because holding is the profitable move. They take coins off the market and
              collect rent from the traders who create volume. Traders, in turn, get a market where most of the supply
              sits with people who are paid not to sell.
            </p>
            <blockquote className="quote">Trading creates rent. Rent keeps supply off the market.</blockquote>
          </div>
        </section>

        <section className="row">
          <h2 className="row-label">A bet on volume</h2>
          <div className="prose">
            <p>
              Dividends come from trading, so a landlord is really betting that volume lasts. A good landlord looks at a coin the way a property investor looks at a neighborhood: not at what it
              rents for today, but at what it will rent for over the next few years.
            </p>
          </div>
        </section>

        <section className="row">
          <h2 className="row-label">Why an endowment</h2>
          <div className="prose">
            <p>
              The endowment turns the landlord&rsquo;s promise into code. It is a landlord that can never sell, funded
              by landlords who choose to reinvest their rent. Its commitment isn&rsquo;t a matter of reputation.
              It&rsquo;s built in.
            </p>
            <p className="muted small">
              Follow <a href={links.x}>@PenisEndowment</a> for launch news.
            </p>
          </div>
        </section>
      </div>
    </>
  );
}
