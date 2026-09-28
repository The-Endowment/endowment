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

/// Hard bounds on the accumulation target, in basis points of supply.
pub const MIN_SUPPLY_TARGET_BPS: u16 = 500;
pub const MAX_SUPPLY_TARGET_BPS: u16 = 4_000;

/// Hard bounds on how much slippage one buyback may cause.
pub const MIN_PRICE_IMPACT_BPS: u16 = 10;
pub const MAX_PRICE_IMPACT_BPS: u16 = 300;
