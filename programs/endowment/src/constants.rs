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
pub const CONFIG_VERSION: u8 = 2;
pub const LANDLORD_VERSION: u8 = 2;

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

/// Buybacks fail closed if the pool's fee (trade + creator) or either mint's
/// transfer fee (current or scheduled) exceeds these.
pub const MAX_POOL_FEE_BPS: u64 = 200;
pub const MAX_TRANSFER_FEE_BPS: u64 = 500;

/// The buyback price floor is measured against the pool's time-weighted average
/// price over at least this window, read from Raydium's observation account.
/// Long enough that the uncertainty in where Raydium's records begin (±7 s,
/// see `raydium::twap_price_x32`) is at most ±0.4% of the average.
pub const TWAP_WINDOW_SECONDS: u64 = 30 * 60;

/// Raydium coalesces swaps within this many seconds of an observation's
/// timestamp into that observation, so a historical observation's cumulative
/// price may run up to this long past its recorded timestamp.
pub const RAYDIUM_OBSERVATION_COALESCE_SECONDS: u64 = 14;

/// No single stretch of the price record counts for more than this share of the
/// TWAP window (a longer one counts at its average price, for that capped time).
/// Raydium prices a stretch at the moment it closes, so a token transfer
/// straight into a pool vault just before a swap colours the whole stretch
/// before it; this caps what one such move can weigh.
pub const MAX_STRETCH_SHARE_BPS: u64 = 5_000;

/// A buyback is refused if the coin's spot price is this far from its TWAP, in
/// either direction.
pub const MAX_SPOT_ABOVE_TWAP_BPS: u64 = 300;

/// Landlord sweeps switch on at `activate_bps` of the coin's supply committed
/// and off below `deactivate_bps`. Both are bounded by this.
pub const MAX_ACTIVATION_BPS: u16 = 5_000;

/// The admin can only renounce once the thresholds are at least this, so sweeps
/// can never be frozen on.
pub const MIN_RENOUNCE_ACTIVATE_BPS: u16 = 1_000;
pub const MIN_RENOUNCE_DEACTIVATE_BPS: u16 = 500;

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
/// freeze the count by starting it and walking away.
pub const COUNT_TIMEOUT_SECS: i64 = 2 * 60 * 60;

/// Sweeps stop if no count has finished for this long (a count that nobody runs
/// can't keep an endowment switched on). Not applied while the activation
/// threshold is 0 (a founders-only test window).
pub const ACTIVE_MAX_AGE_SECS: i64 = 3 * 24 * 60 * 60;

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
