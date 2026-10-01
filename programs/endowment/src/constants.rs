use anchor_lang::prelude::*;

/// Config PDA seeds: [CONFIG_SEED, coin_mint, creator]. One endowment per
/// (coin, creator) pair; the creator in the seeds means nobody can squat
/// another creator's instance.
#[constant]
pub const CONFIG_SEED: &[u8] = b"config";

/// Authority PDA seeds: [AUTHORITY_SEED, config]. Owns the instance's vaults and
/// is the delegate its landlords approve on their dividend account.
#[constant]
pub const AUTHORITY_SEED: &[u8] = b"authority";

/// Landlord PDA seeds: [LANDLORD_SEED, config, owner].
#[constant]
pub const LANDLORD_SEED: &[u8] = b"landlord";

/// Account layout versions, for future migrations.
pub const CONFIG_VERSION: u8 = 3;
pub const LANDLORD_VERSION: u8 = 3;

/// A guardian pause lifts on its own after this long, and a new pause can only
/// start this long after the last one ended. The guardian can therefore stop an
/// endowment at most half the time, and never for good.
pub const MAX_PAUSE_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const PAUSE_COOLDOWN_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Parameter changes (and retiring) wait this long between proposal and effect.
pub const PARAM_TIMELOCK_SECONDS: i64 = 72 * 60 * 60;

/// Once a proposal matures, only the admin may apply it for this long, so the
/// admin can still cancel it (or cancel and renounce) without being raced.
/// After that anyone may apply it, until it expires.
pub const PARAM_APPLY_GRACE_SECONDS: i64 = 24 * 60 * 60;
pub const PARAM_EXPIRY_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Hard bounds on how much slippage one buyback may cause.
pub const MIN_PRICE_IMPACT_BPS: u16 = 10;
pub const MAX_PRICE_IMPACT_BPS: u16 = 300;

/// Hard bounds on `max_twap_deviation_bps`: how far the spot price may sit from
/// the TWAP, in either direction, for a buyback to run. The same value is the
/// floor's allowance for that drift, so it is also the most a permissionless
/// caller can make a buy lose to it; hence the ceiling. (Round 3's replay of
/// nine days of this pair's prices: ±3% allowed buys about half the time,
/// ±5% about 78%.)
pub const MIN_TWAP_DEVIATION_BPS: u16 = 100;
pub const MAX_TWAP_DEVIATION_BPS: u16 = 1_000;

/// Buybacks fail closed if the pool's fee (trade + creator) or either mint's
/// transfer fee (current or scheduled) exceeds these.
pub const MAX_POOL_FEE_BPS: u64 = 200;
pub const MAX_TRANSFER_FEE_BPS: u64 = 500;

/// The buyback price floor is measured against the pool's time-weighted average
/// price, read from Raydium's observation account, over this window when the
/// record holds that much history.
pub const TWAP_WINDOW_SECONDS: u64 = 30 * 60;

/// ...and never over less than this. Raydium's record is a ring of 100
/// observations at least 15 s apart, so it always spans at least 99 × 15 s =
/// 1,485 s: a busy pool (or a flood of dust swaps) can shorten the window to
/// what the ring holds, but never below this minimum, so frequent trading can't
/// make the TWAP unavailable (R3-TW-01). Shorter would make the average cheaper
/// to push; the reader's timing uncertainty (see `MAX_TWAP_READER_ERROR_BPS`)
/// weighs more as the window shrinks.
pub const MIN_TWAP_WINDOW_SECONDS: u64 = 15 * 60;

/// Raydium coalesces swaps within this many seconds of an observation's
/// timestamp into that observation, so a historical observation's cumulative
/// price may run up to this long past its recorded timestamp.
pub const RAYDIUM_OBSERVATION_COALESCE_SECONDS: u64 = 14;

/// The reader takes each older record as of the middle of that range (±7 s) and
/// works out, for the stretches it actually used, a bound on how far that timing
/// uncertainty can move the average (`raydium::twap_price_x32`): about ±7 s over
/// the window when no stretch is capped, more for each capped stretch, whose
/// endpoints no longer cancel against their neighbours' (FC-R3-01). The floor
/// and the spot band allow for that computed bound, so a flat price never reads
/// as a loss. A history whose bound exceeds this is refused as unreadable: it
/// takes several capped stretches with swaps timed around them, which is also
/// what a manipulated record looks like.
pub const MAX_TWAP_READER_ERROR_BPS: u64 = 300;

/// Added to the computed reader bound for integer rounding in the floor.
pub const TWAP_ROUNDING_BPS: u64 = 10;

/// No single stretch of the price record counts for more than this share of the
/// full TWAP window (a longer one counts at its average price, for that capped
/// time). Raydium prices a stretch at the moment it closes, so a token transfer
/// straight into a pool vault just before a swap colours the whole stretch
/// before it; this caps what one such move can weigh: a quarter, so an attacker
/// needs four coloured stretches to own the average (R3-TW-04).
pub const MAX_STRETCH_SHARE_BPS: u64 = 2_500;

/// Landlord sweeps switch on at `activate_bps` of the coin's supply committed
/// and off below `deactivate_bps`. Both are bounded by this.
pub const MAX_ACTIVATION_BPS: u16 = 5_000;

/// The admin can only renounce once the thresholds are at least this, so sweeps
/// can never be frozen on.
pub const MIN_RENOUNCE_ACTIVATE_BPS: u16 = 1_000;
pub const MIN_RENOUNCE_DEACTIVATE_BPS: u16 = 500;

/// Whenever the activation threshold is above 0, the deactivation threshold
/// must be at least this, so a count that finds nothing always switches sweeps
/// off (R3-RF-04). A threshold of 0/0 is a founders-only test window.
pub const MIN_DEACTIVATE_BPS: u16 = 1;

/// A landlord must hold at least `min_stake_bps` of the coin's supply to register
/// and to be counted. Bounded by this.
pub const MAX_MIN_STAKE_BPS: u16 = 500;

/// A landlord counts toward activation only while its dividend account still
/// delegates at least this much to the endowment (the website approves u64::MAX).
pub const MIN_DELEGATION: u64 = u64::MAX / 2;

/// A commitment count can start at most once per this many seconds.
pub const COUNT_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Once a count has been open this long (not counting any pause), anyone can
/// finish it; landlords not yet counted count as zero for that round. Nobody can
/// freeze the count by starting it and walking away. Long enough for the
/// refresher to complete its reads of landlords left pending (see below).
pub const COUNT_TIMEOUT_SECS: i64 = 4 * 60 * 60;

/// A landlord only counts after this many reads by the endowment's refresher
/// since its last count read, each at least `MIN_ATTEST_SPACING_SECS` after the
/// previous one. Each read lowers the landlord's recorded balance to what it
/// holds at that moment, so to have one holding counted in several wallets an
/// attacker must have it in each wallet at each of that wallet's reads: it must
/// win the race against every one of several reads at times it doesn't choose,
/// not just one (R3-RF-02).
pub const REQUIRED_ATTESTATIONS: u8 = 3;
pub const MIN_ATTEST_SPACING_SECS: i64 = 30 * 60;

/// Neither a sweep nor a donation takes a dividend vault above this many days
/// of what it can spend (`Config::vault_cap`): what's already in the vault is
/// what could be stranded if a third-party authority (a fee or hook authority,
/// say) or a pool halt stopped buybacks for good, so it's kept to a few days'
/// worth (R3-MINT-01).
pub const MAX_VAULT_DAYS_OF_BUYS: u64 = 3;

/// Sweeps stop if no count has finished for this long (a count that nobody runs
/// can't keep an endowment switched on). Not applied while the activation
/// threshold is 0 (a founders-only test window).
pub const ACTIVE_MAX_AGE_SECS: i64 = 3 * 24 * 60 * 60;

/// Reward allowance (`post_reward_total`): a landlord can be swept at most what
/// its counted coin earned, times `allowance_margin_bps`. The margin is 0 (off:
/// sweeps take everything above the baseline) or between these bounds.
pub const MIN_ALLOWANCE_MARGIN_BPS: u16 = 10_000;
pub const MAX_ALLOWANCE_MARGIN_BPS: u16 = 30_000;

/// `Config::reward_index` is dividend earned per coin base unit, scaled by this.
pub const REWARD_INDEX_SCALE: u128 = 1_000_000_000_000;

/// The refresher can post the coin's reward total at most this often.
pub const MIN_REWARD_POST_SPACING_SECS: i64 = 60 * 60;

/// Minimum time between buybacks, bounds.
pub const MIN_BUY_INTERVAL_BOUNDS: (i64, i64) = (60, 24 * 60 * 60);

/// Tip paid to whoever cranks a buyback, in basis points of the amount spent.
pub const MAX_TIP_BPS: u16 = 50;

/// Optional donation to the flagship endowment, chosen at creation and locked.
pub const ALLOWED_DONATION_BPS: [u16; 4] = [0, 10, 20, 30];

/// The tip and the donation both come out of each buyback; together they are capped.
pub const MAX_TIP_PLUS_DONATION_BPS: u16 = 80;

/// The flagship endowment ($PENIS). Its config address is deterministic:
/// PDA([CONFIG_SEED, FLAGSHIP_COIN_MINT, FLAGSHIP_CREATOR]) under this program,
/// so it is fixed before the flagship exists. Donations go to its dividend
/// vault: the associated token account of `authority(flagship_config())` for
/// the dividend mint. See `flagship_config()`.
pub const FLAGSHIP_COIN_MINT: Pubkey = pubkey!("JE3HT7SbCgXDQWV6xp3oiiAisDzq4HyZ8wyEVBDCs45Z");

/// The wallet that will sign `create_endowment` for the flagship.
///
/// SET BEFORE DEPLOY: replace with the flagship creator's address. While this is
/// the all-zero placeholder, no endowment can choose a donation (see
/// `validate_donation`), so nothing can ever be sent to an unowned account.
#[cfg(not(feature = "test-flagship"))]
pub const FLAGSHIP_CREATOR: Pubkey = Pubkey::new_from_array([0; 32]);

/// A fixed test creator, only in builds made for the integration tests
/// (`--features test-flagship`). Never deploy such a build.
#[cfg(feature = "test-flagship")]
pub const FLAGSHIP_CREATOR: Pubkey = pubkey!("HPbBWhYdj1s4T9rbN7v6CLmwxZY7aoNbfEaZwgENaSJH");

/// Donations are only possible for endowments whose dividend is the flagship's
/// dividend asset (PUMP), so the donation can be spent by the flagship directly.
pub const FLAGSHIP_DIVIDEND_MINT: Pubkey = pubkey!("pumpCmXqMfrsAkQ5r49WcJnRayYRqmXz6ae8H7H9Dfn");

/// Whether the flagship creator has been set (see FLAGSHIP_CREATOR).
pub fn flagship_is_set() -> bool {
    FLAGSHIP_CREATOR != Pubkey::default()
}

/// The flagship endowment's config address.
pub fn flagship_config() -> Pubkey {
    Pubkey::find_program_address(
        &[CONFIG_SEED, FLAGSHIP_COIN_MINT.as_ref(), FLAGSHIP_CREATOR.as_ref()],
        &crate::ID,
    )
    .0
}
