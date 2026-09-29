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

/// A guardian pause lifts on its own after this long.
pub const MAX_PAUSE_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Hard bounds on how much slippage one buyback may cause.
pub const MIN_PRICE_IMPACT_BPS: u16 = 10;
pub const MAX_PRICE_IMPACT_BPS: u16 = 300;

/// Landlord sweeps switch on at `activate_bps` of the coin's supply committed
/// and off below `deactivate_bps`. Both are bounded by this.
pub const MAX_ACTIVATION_BPS: u16 = 5_000;

/// A commitment count can start at most once per this many seconds.
pub const COUNT_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Minimum time between buybacks, bounds.
pub const MIN_BUY_INTERVAL_BOUNDS: (i64, i64) = (60, 24 * 60 * 60);

/// Tip paid to whoever cranks a buyback, in basis points of the amount spent.
pub const MAX_TIP_BPS: u16 = 50;

/// Optional donation to the flagship endowment, chosen at creation and locked.
pub const ALLOWED_DONATION_BPS: [u16; 4] = [0, 10, 20, 30];

/// The tip and the donation both come out of each buyback; together they are capped.
pub const MAX_TIP_PLUS_DONATION_BPS: u16 = 80;

/// The flagship endowment ($PENIS). Donations go to its dividend vault: the
/// associated token account of `authority(FLAGSHIP_CONFIG)` for the dividend mint.
///
/// PLACEHOLDER: set this to the real $PENIS instance's config address after that
/// instance is created on mainnet, and redeploy, before the upgrade authority is
/// burned. Until then, donations land in an account nobody controls.
pub const FLAGSHIP_CONFIG: Pubkey = Pubkey::new_from_array([0xF1; 32]);

/// Donations are only possible for endowments whose dividend is the flagship's
/// dividend asset (PUMP), so the donation can be spent by the flagship directly.
pub const FLAGSHIP_DIVIDEND_MINT: Pubkey = pubkey!("pumpCmXqMfrsAkQ5r49WcJnRayYRqmXz6ae8H7H9Dfn");
