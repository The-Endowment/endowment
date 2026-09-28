import Link from "next/link";
import { CopyAddress } from "@/components/CopyAddress";
import { DotLogo } from "@/components/Logo";
import { links, PENIS_MINT } from "@/lib/site";

const splits = [
  { condition: "Until it holds 20% of supply", buyback: "95%", liquidity: "0%", marketing: "up to 5%", current: true },
  { condition: "Healthy market", buyback: "85%", liquidity: "10%", marketing: "5%" },
  { condition: "Pool too thin", buyback: "40%", liquidity: "50%", marketing: "10%" },
  { condition: "Volume falling", buyback: "60%", liquidity: "10%", marketing: "30%" },
];

const faqs = [
  {
    q: "What is PUMP?",
    a: "The token of pump.fun. Every $PENIS trade pays a fee, and that fee is paid out to every $PENIS holder in PUMP. The endowment runs on that income.",
  },
  {
    q: "Is my wallet safe if I opt in?",
    a: "You delegate one token account: your PUMP. The endowment can move only new PUMP that lands there after you opt in. It can't touch your $PENIS, your SOL, any other token, or PUMP you already held.",
  },
  {
    q: "Can I leave?",
    a: "Yes, anytime. Revoke the delegation from any Solana wallet. It's a standard token instruction, so it works even if this site is down or the endowment is paused.",
  },
  {
    q: "Who runs it?",
    a: "The landlords, meaning the largest holders. The pause switch and admin controls will be multisigs held by landlords, and every action is posted publicly. The endowment has no connection to the coin's creators.",
  },
  {
    q: "Has it been audited?",
    a: "Not yet. The source is public, it will be reviewed before launch, and it starts with small spending caps.",
  },
  {
    q: "When does it open?",
    a: "Once the landlords agree on the final rules. It will open to the founding landlords first, then to everyone.",
  },
];

export default function Home() {
  return (
    <>
      <div className="wrap">
      <section className="hero">
        <div className="hero-copy">
          <h1 className="display h1">
            Permanent capital.
            <br />
            <em>Firm</em> commitments.
          </h1>
          <p className="lede">
            The $PENIS Endowment holds $PENIS forever and spends only the PUMP it earns, on buybacks, liquidity and
            growth. It is the one holder that can never pull out.
          </p>
          <div className="actions">
            <Link href="#how" className="button button-primary">
              How it works
            </Link>
            <a href={links.github} className="button">
              Read the contract
            </a>
          </div>
        </div>
        <DotLogo className="hero-art" label="The endowment's mark, a temple drawn in dots" />
      </section>
        <section id="how" className="row">
          <h2 className="row-label">How it works</h2>
          <div className="row-body">
            <h3 className="statement">Landlords lend their rent. The endowment keeps the building.</h3>
            <p>
              The largest holders, the landlords, lend the endowment their PUMP rewards. The endowment turns that
              income into $PENIS it can never sell.
            </p>
            <ol className="steps">
              <li>
                <h3>Delegate</h3>
                <p>One transaction gives the endowment access to your PUMP rewards and nothing else.</p>
              </li>
              <li>
                <h3>Sweep</h3>
                <p>When dividends land, the new PUMP moves to the endowment within seconds.</p>
              </li>
              <li>
                <h3>Compound</h3>
                <p>It buys $PENIS that stays locked. That $PENIS earns dividends too, which buy more.</p>
              </li>
            </ol>
          </div>
        </section>

        <section className="row">
          <h2 className="row-label">How it spends</h2>
          <div className="row-body">
            <h3 className="statement">It reads the market, then spends.</h3>
            <p>
              First it accumulates. Once it holds a fifth of all $PENIS, it becomes a thermostat: each day it reads how
              deep the trading pool is and how much PUMP it earned, which tracks volume, and splits the day&rsquo;s
              income by fixed, public rules.
            </p>
            <div className="figures">
              <div className="figure">
                <span className="figure-value">200,000,000</span>
                <span className="figure-label">$PENIS to accumulate first</span>
              </div>
              <div className="figure">
                <span className="figure-value">20%</span>
                <span className="figure-label">of all $PENIS, held forever</span>
              </div>
            </div>
            <table className="table">
              <thead>
                <tr>
                  <th>When</th>
                  <th className="num">Buyback</th>
                  <th className="num">Liquidity</th>
                  <th className="num">Marketing</th>
                </tr>
              </thead>
              <tbody>
                {splits.map((s) => (
                  <tr key={s.condition} className={s.current ? "current" : undefined}>
                    <td>
                      {s.condition}
                      {s.current && <span className="tag">first</span>}
                    </td>
                    <td className="num">{s.buyback}</td>
                    <td className="num">{s.liquidity}</td>
                    <td className="num">{s.marketing}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>

        <section className="row">
          <h2 className="row-label">Guarantees</h2>
          <div className="row-body">
            <h3 className="statement">Written into the contract, not promised in a thread.</h3>
            <ul className="plain-list">
              <li>
                <strong>It never sells</strong>
                <span>The contract has no function that can sell or withdraw its $PENIS.</span>
              </li>
              <li>
                <strong>You can always leave</strong>
                <span>Revoking works from any wallet, even if this site is down.</span>
              </li>
              <li>
                <strong>The pause is limited</strong>
                <span>It can&rsquo;t move funds, and it lifts on its own after seven days.</span>
              </li>
              <li>
                <strong>Everything is public</strong>
                <span>
                  The <a href={links.github}>source code</a> is open, and every action is posted to{" "}
                  <a href={links.x}>@PenisEndowment</a>.
                </span>
              </li>
            </ul>
          </div>
        </section>

        <section className="band">
          <blockquote>
            Trading creates rent. <span>Rent keeps supply off the market.</span>
          </blockquote>
          <Link href="/thesis" className="muted">
            Read the landlord thesis
          </Link>
        </section>

        <section id="coin" className="row">
          <h2 className="row-label">The coin</h2>
          <div className="row-body">
            <h3 className="statement">$PENIS pays rent in PUMP.</h3>
            <p>
              $PENIS is a meme coin on Solana, launched on stonk.fun. Every trade pays a fee, and holders receive it as
              PUMP. It is the oldest joke there is with the best possible ticker, and a penis that pays you in PUMP.
            </p>
            <dl className="facts">
              <div className="fact">
                <dt>Mint address</dt>
                <dd>
                  <CopyAddress address={PENIS_MINT} />
                </dd>
              </div>
              <div className="fact">
                <dt>Supply</dt>
                <dd>About 1 billion. Minting and freezing are permanently disabled.</dd>
              </div>
              <div className="fact">
                <dt>Dividends</dt>
                <dd>A 3% fee on every transfer, paid to holders in PUMP</dd>
              </div>
              <div className="fact">
                <dt>Main market</dt>
                <dd>
                  PENIS/PUMP on Raydium. <a href={links.dexscreener}>Chart</a>, <a href={links.solscan}>Solscan</a>
                </dd>
              </div>
            </dl>
            <div className="actions">
              <a href={links.stonkfun} className="button">
                $PENIS on stonk.fun
              </a>
            </div>
          </div>
        </section>

        <section id="status" className="row">
          <h2 className="row-label">Status</h2>
          <div className="row-body">
            <h3 className="statement">Built, tested, and waiting on the landlords.</h3>
            <p>
              The contract is written and tested. Delegation opens once the landlords agree on the final rules,
              starting with the founding landlords and small caps. The leaderboard and the full ledger go live the same
              day. Follow <a href={links.x}>@PenisEndowment</a> for the announcement.
            </p>
          </div>
        </section>

        <section id="questions" className="row">
          <h2 className="row-label">Questions</h2>
          <div className="row-body">
            <div className="faq">
              {faqs.map((f) => (
                <details key={f.q}>
                  <summary>{f.q}</summary>
                  <p>{f.a}</p>
                </details>
              ))}
            </div>
          </div>
        </section>
      </div>
    </>
  );
}
