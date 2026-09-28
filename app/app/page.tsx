import Link from "next/link";
import { CopyAddress } from "@/components/CopyAddress";
import { DotLogo } from "@/components/Logo";
import { links, PENIS_MINT } from "@/lib/site";

const splits = [
  { condition: "Accumulating to 20%", buyback: "95%", liquidity: "0%", marketing: "≤5%", current: true },
  { condition: "Healthy", buyback: "85%", liquidity: "10%", marketing: "5%" },
  { condition: "Pool too thin", buyback: "40%", liquidity: "50%", marketing: "10%" },
  { condition: "Volume falling", buyback: "60%", liquidity: "10%", marketing: "30%" },
];

const faqs = [
  {
    q: "What is PUMP?",
    a: "PUMP is the token of pump.fun. Every $PENIS trade pays a fee, and that fee is paid out to every $PENIS holder in PUMP. The endowment runs on that income.",
  },
  {
    q: "Is my wallet safe if I opt in?",
    a: "You delegate one thing: your PUMP token account. The endowment can move only the new PUMP that lands there after you opt in. It can never touch your $PENIS, your SOL, any other token, or the PUMP you already held.",
  },
  {
    q: "Can I leave?",
    a: "Anytime. Revoke the delegation from any Solana wallet. It's a standard token instruction, so it works even if this site is down or the endowment is paused.",
  },
  {
    q: "Who runs it?",
    a: "Landlords: the largest $PENIS holders. The emergency pause and the admin controls will be multisigs held by landlords, and every action is posted publicly. The endowment is independent of the coin's creators.",
  },
  {
    q: "Has it been audited?",
    a: "Not yet. The code is open source for anyone to read, it will be reviewed before launch, and it starts with small spending caps.",
  },
  {
    q: "When does it open?",
    a: "After the landlords sign off on the final rules. Follow @PenisEndowment for the announcement.",
  },
];

export default function Home() {
  return (
    <>
      <section className="wrap hero">
        <div className="hero-copy">
          <div className="eyebrow">Est. 2026 · On Solana</div>
          <h1 className="display h1">
            Permanent capital.
            <br />
            <em className="display-em">Firm</em> commitments.
          </h1>
          <p className="lede">
            The endowment holds $PENIS forever and spends only the PUMP it earns: on buybacks, liquidity, and
            extended growth. The only holder that never pulls out.
          </p>
          <div className="actions">
            <Link href="#delegate" className="button button-primary">
              Delegate your PUMP
            </Link>
            <a href={links.github} className="button">
              Read the contract
            </a>
          </div>
        </div>
        <DotLogo className="hero-art" label="The endowment's logo: a temple drawn in dots" />
      </section>

      <section className="wrap" aria-label="Holdings">
        <div className="stats">
          <div className="stat">
            <div className="stat-label">Assets under management</div>
            <div className="stat-value">—</div>
            <div className="stat-note">$PENIS held, locked forever</div>
          </div>
          <div className="stat">
            <div className="stat-label">Progress to target</div>
            <div className="stat-value">0%</div>
            <div className="bar" aria-hidden="true">
              <span style={{ width: "0%" }} />
            </div>
            <div className="stat-note">of 200,000,000 $PENIS (20% of supply)</div>
          </div>
          <div className="stat">
            <div className="stat-label">PUMP received</div>
            <div className="stat-value">—</div>
            <div className="stat-note">from landlords and its own dividends</div>
          </div>
          <div className="stat">
            <div className="stat-label">Landlords committed</div>
            <div className="stat-value">—</div>
            <div className="stat-note">Live once the endowment opens</div>
          </div>
        </div>
      </section>

      <section id="how" className="wrap section">
        <div className="section-head">
          <h2 className="display h2">How it works</h2>
          <p>Landlords lend the endowment their PUMP rewards. It turns them into $PENIS that can never be sold.</p>
        </div>
        <div className="grid-3">
          <div className="card step">
            <div className="step-num">01</div>
            <h3 className="display h3">Delegate</h3>
            <p>Sign one transaction. The endowment can move your PUMP rewards and nothing else. Your $PENIS and SOL stay untouchable.</p>
          </div>
          <div className="card step">
            <div className="step-num">02</div>
            <h3 className="display h3">Sweep</h3>
            <p>When dividends land, only the new PUMP moves to the endowment, within seconds. What you held before stays yours.</p>
          </div>
          <div className="card step">
            <div className="step-num">03</div>
            <h3 className="display h3">Compound</h3>
            <p>It buys $PENIS that can never leave. That $PENIS earns dividends too, which buy more. Growth, extended.</p>
          </div>
        </div>
      </section>

      <section className="wrap section grid-2" style={{ alignItems: "start", gap: "clamp(32px, 5vw, 64px)" }}>
        <div style={{ display: "flex", flexDirection: "column", gap: 24 }}>
          <div className="eyebrow">The thermostat</div>
          <h2 className="display h2">It reads the market, then spends.</h2>
          <p className="muted" style={{ margin: 0 }}>
            Each day it checks two numbers on-chain: how deep the pool is, and how much PUMP it earned, which tracks
            volume. Then it splits the day&rsquo;s income by fixed public rules.
          </p>
          <div className="chips">
            <span className="chip chip-on">
              <span className="chip-dot" />
              Accumulating to 20%
            </span>
            <span className="chip">Healthy</span>
            <span className="chip">Pool too thin</span>
            <span className="chip">Volume falling</span>
          </div>
        </div>
        <div className="card" style={{ paddingTop: 12, paddingBottom: 12 }}>
          <table className="table">
            <thead>
              <tr>
                <th>Condition</th>
                <th className="num">Buyback</th>
                <th className="num">Liquidity</th>
                <th className="num">Marketing</th>
              </tr>
            </thead>
            <tbody>
              {splits.map((s) => (
                <tr key={s.condition} className={s.current ? "current" : undefined}>
                  <td>{s.condition}</td>
                  <td className="num">{s.buyback}</td>
                  <td className="num">{s.liquidity}</td>
                  <td className="num">{s.marketing}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section id="coin" className="wrap section grid-2" style={{ alignItems: "start", gap: "clamp(32px, 5vw, 64px)" }}>
        <div style={{ display: "flex", flexDirection: "column", gap: 24 }}>
          <div className="eyebrow">About the coin</div>
          <h2 className="display h2">$PENIS pays rent in PUMP.</h2>
          <p className="muted" style={{ margin: 0 }}>
            $PENIS is a meme coin on Solana, launched on stonk.fun, that pays its holders a share of every trade in
            PUMP. The endowment is one of those holders, and it never sells.
          </p>
          <ul className="muted" style={{ margin: 0, paddingLeft: 20, display: "flex", flexDirection: "column", gap: 8 }}>
            <li>The most universally understood joke on earth, with a perfect ticker.</li>
            <li>A penis that pays in PUMP: a parody of the pump economy that also feeds it.</li>
            <li>Real yield, straight from trading, paid automatically.</li>
          </ul>
          <div className="actions">
            <a href={links.stonkfun} className="button button-primary">
              $PENIS on stonk.fun
            </a>
            <a href={links.dexscreener} className="button">
              Chart
            </a>
            <a href={links.solscan} className="button">
              Solscan
            </a>
          </div>
        </div>
        <div className="card">
          <dl className="facts" style={{ margin: 0 }}>
            <div className="fact">
              <dt>Mint address</dt>
              <dd>
                <CopyAddress address={PENIS_MINT} />
              </dd>
            </div>
            <div className="fact">
              <dt>Supply</dt>
              <dd className="mono">~1,000,000,000</dd>
            </div>
            <div className="fact">
              <dt>Dividends</dt>
              <dd>3% fee on every transfer, paid out in PUMP</dd>
            </div>
            <div className="fact">
              <dt>Main pool</dt>
              <dd>PENIS / PUMP on Raydium</dd>
            </div>
            <div className="fact">
              <dt>Mint and freeze</dt>
              <dd>Both revoked. No new $PENIS, ever.</dd>
            </div>
          </dl>
        </div>
      </section>

      <section id="delegate" className="wrap section">
        <div className="card cta" style={{ padding: "clamp(28px, 5vw, 64px)", borderColor: "var(--accent)" }}>
          <div style={{ display: "flex", flexDirection: "column", gap: 12, maxWidth: 720 }}>
            <div className="eyebrow">Delegation opens soon</div>
            <h2 className="display h3">The contract is built. The landlords are signing off.</h2>
            <p className="muted" style={{ margin: 0 }}>
              Delegation opens once the landlords agree on the final rules. It starts with the founding landlords and
              small caps, then opens to everyone.
            </p>
          </div>
          <a href={links.x} className="button button-primary">
            Follow @PenisEndowment
          </a>
        </div>
      </section>

      <section className="wrap section grid-2">
        <div className="card" style={{ display: "flex", flexDirection: "column", gap: 20 }}>
          <h2 className="display h3">Top landlords</h2>
          <div className="empty">The leaderboard fills in when the endowment opens.</div>
        </div>
        <div className="card" style={{ display: "flex", flexDirection: "column", gap: 20 }}>
          <h2 className="display h3">The ledger</h2>
          <div className="empty">Every sweep, buyback, and spend will be listed here, each linked on-chain.</div>
        </div>
      </section>

      <section className="wrap section">
        <h2 className="display h2" style={{ marginBottom: 48 }}>
          Written into the contract
        </h2>
        <div className="grid-4">
          <div className="guarantee">
            <h3>It never pulls out</h3>
            <p>There is no function that can sell or withdraw the $PENIS it holds.</p>
          </div>
          <div className="guarantee">
            <h3>Leave anytime</h3>
            <p>Revoke from any wallet. It works even if this site is down.</p>
          </div>
          <div className="guarantee">
            <h3>A short leash</h3>
            <p>The pause switch can&rsquo;t move funds, and it lifts on its own after 7 days.</p>
          </div>
          <div className="guarantee">
            <h3>Fully exposed</h3>
            <p>Open source, verifiable on-chain, and every action posted publicly.</p>
          </div>
        </div>
      </section>

      <section id="faq" className="wrap section">
        <h2 className="display h2" style={{ marginBottom: 32 }}>
          Questions
        </h2>
        <div className="faq">
          {faqs.map((f) => (
            <details key={f.q}>
              <summary>{f.q}</summary>
              <p>{f.a}</p>
            </details>
          ))}
        </div>
      </section>
    </>
  );
}
