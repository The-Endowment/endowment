use anchor_lang::prelude::*;

#[constant]
pub const CONFIG_SEED: &[u8] = b"config";

/// Owns every vault and is the delegate landlords approve on their PUMP account.
#[constant]
pub const AUTHORITY_SEED: &[u8] = b"authority";

#[constant]
pub const LANDLORD_SEED: &[u8] = b"landlord";

/// A guardian pause lifts on its own after this long.
pub const MAX_PAUSE_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Hard bounds on how much slippage one buyback may cause.
pub const MIN_PRICE_IMPACT_BPS: u16 = 10;
pub const MAX_PRICE_IMPACT_BPS: u16 = 300;

/// Landlord sweeps switch on at `activate_bps` of $PENIS supply committed and
/// off below `deactivate_bps`. Both are bounded by this.
pub const MAX_ACTIVATION_BPS: u16 = 5_000;
pub const DEFAULT_ACTIVATE_BPS: u16 = 3_000;
pub const DEFAULT_DEACTIVATE_BPS: u16 = 2_500;

/// A commitment count can start at most once per this many seconds.
pub const COUNT_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Landlord contributions close for good once the $PENIS vault holds this much
/// (200M $PENIS at 6 decimals).
pub const CONTRIBUTION_CAP: u64 = 200_000_000 * 1_000_000;

/// Share of each post-close buyback that buys $PENIS; the rest becomes locked liquidity.
pub const DEFAULT_BUY_BPS: u16 = 10_000;

/// Minimum time between buybacks.
pub const DEFAULT_MIN_BUY_INTERVAL_SECS: i64 = 600;
pub const MIN_BUY_INTERVAL_BOUNDS: (i64, i64) = (60, 24 * 60 * 60);

/// Tip paid to whoever cranks a buyback, in basis points of the PUMP spent.
pub const DEFAULT_TIP_BPS: u16 = 25;
pub const MAX_TIP_BPS: u16 = 50;
