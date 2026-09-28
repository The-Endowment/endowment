//! Buyback price protection.
//!
//! The endowment will not accept a fill worse than
//!
//!   floor = spot_out × (1 − transfer_fee) × (1 − pool_fee − max_price_impact)
//!
//! where `spot_out = amount_in × reserve_out / reserve_in` is what the trade
//! would return at the pool's current price with no slippage and no fees.
//! `transfer_fee` is $PENIS's Token-2022 fee (3%), withheld from what lands in
//! the vault; `pool_fee` is Raydium's fee on the input. What remains,
//! `max_price_impact`, bounds the slippage this one trade is allowed to cause,
//! so a manipulated or thin pool can't be used to drain the vault. The realized
//! amount is measured from the vault balance, so the check uses what actually
//! arrived.

const BPS: u128 = 10_000;

/// Minimum acceptable $PENIS received for `amount_in` PUMP.
pub fn min_acceptable_out(
    amount_in: u64,
    reserve_in: u64,
    reserve_out: u64,
    pool_fee_bps: u64,
    transfer_fee_bps: u64,
    max_price_impact_bps: u64,
) -> Option<u64> {
    if reserve_in == 0 {
        return None;
    }
    let spot_out = (amount_in as u128).checked_mul(reserve_out as u128)? / reserve_in as u128;
    let after_transfer_fee = spot_out * BPS.checked_sub(transfer_fee_bps as u128)? / BPS;
    let allowance = BPS.checked_sub(pool_fee_bps as u128 + max_price_impact_bps as u128)?;
    u64::try_from(after_transfer_fee * allowance / BPS).ok()
}

/// Starts a new 24-hour window once the current one has elapsed.
/// Returns (day_start, bought_today).
pub fn roll_day(day_start: i64, bought_today: u64, now: i64) -> (i64, u64) {
    if now.saturating_sub(day_start) >= 24 * 60 * 60 {
        (now, 0)
    } else {
        (day_start, bought_today)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_applies_every_haircut() {
        // 1,000 in at 7:1 = 7,000 spot. 3% transfer fee → 6,790.
        // 25 bps pool fee + 100 bps impact allowance → 6,790 × 0.9875 = 6,705.
        assert_eq!(min_acceptable_out(1_000, 10_000_000, 70_000_000, 25, 300, 100), Some(6_705));
    }

    #[test]
    fn rejects_empty_pools_and_impossible_allowances() {
        assert_eq!(min_acceptable_out(1_000, 0, 70_000_000, 25, 300, 100), None);
        assert_eq!(min_acceptable_out(1_000, 1, 1, 9_950, 0, 100), None);
    }

    #[test]
    fn day_window_rolls_after_24_hours() {
        assert_eq!(roll_day(100, 50, 100 + 86_399), (100, 50));
        assert_eq!(roll_day(100, 50, 100 + 86_400), (86_500, 0));
    }
}
