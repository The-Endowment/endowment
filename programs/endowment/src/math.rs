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

/// How much of the dividend one buyback spends, decided by the contract:
/// whatever the vault holds (leaving room for the tip and any donation, both
/// charged on top as `extra_bps` of the amount), capped per transaction and by
/// what's left of today's cap.
pub fn buy_amount(vault_balance: u64, extra_bps: u16, max_per_tx: u64, left_today: u64) -> u64 {
    let spendable = (vault_balance as u128 * BPS / (BPS + extra_bps as u128)) as u64;
    spendable.min(max_per_tx).min(left_today)
}

/// Splits a buyback: `buy_bps` buys $PENIS; the rest becomes liquidity, half
/// of it swapped to $PENIS first. Returns (pump_to_swap, pump_to_deposit, liquidity_swap_share).
pub fn split_buy(amount_in: u64, buy_bps: u16) -> (u64, u64, u64) {
    let buy_part = (amount_in as u128 * buy_bps as u128 / BPS) as u64;
    let lp_part = amount_in - buy_part;
    let lp_swap = lp_part / 2;
    (buy_part + lp_swap, lp_part - lp_swap, lp_swap)
}

/// LP tokens to request so Raydium's rounded-up deposit fits within what we
/// have. `penis_net` is $PENIS available after its transfer fee.
pub fn lp_tokens_for(pump: u64, penis_net: u64, reserve_pump: u64, reserve_penis: u64, lp_supply: u64) -> u64 {
    if reserve_pump == 0 || reserve_penis == 0 || lp_supply == 0 {
        return 0;
    }
    let by_pump = pump as u128 * lp_supply as u128 / reserve_pump as u128;
    let by_penis = penis_net as u128 * lp_supply as u128 / reserve_penis as u128;
    // One unit of headroom for Raydium's ceiling rounding.
    (by_pump.min(by_penis).saturating_sub(1)).min(u64::MAX as u128) as u64
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
    fn buy_amount_leaves_room_for_the_tip_and_respects_caps() {
        // 10,025 in the vault at a 25 bps tip: 10,000 spent + 25 tip.
        assert_eq!(buy_amount(10_025, 25, u64::MAX, u64::MAX), 10_000);
        assert_eq!(buy_amount(10_025, 25, 4_000, u64::MAX), 4_000);
        assert_eq!(buy_amount(10_025, 25, 4_000, 1_500), 1_500);
        assert_eq!(buy_amount(0, 25, 4_000, 1_500), 0);
        // A 25 bps tip plus a 30 bps donation: 10,055 covers 10,000 + 25 + 30.
        assert_eq!(buy_amount(10_055, 55, u64::MAX, u64::MAX), 10_000);
    }

    #[test]
    fn split_sends_the_liquidity_share_half_to_the_swap() {
        assert_eq!(split_buy(1_000, 10_000), (1_000, 0, 0));
        // 70% buy: 700 buys, 300 to liquidity (150 swapped, 150 deposited).
        assert_eq!(split_buy(1_000, 7_000), (850, 150, 150));
        assert_eq!(split_buy(1_001, 0), (500, 501, 500));
    }

    #[test]
    fn lp_request_fits_both_sides() {
        // Pool 1,000 PUMP : 7,000 PENIS, 100 LP supply. 10 PUMP + 70 PENIS → 1 LP, minus headroom.
        assert_eq!(lp_tokens_for(100, 700, 1_000, 7_000, 1_000), 99);
        // The scarcer side limits it.
        assert_eq!(lp_tokens_for(100, 350, 1_000, 7_000, 1_000), 49);
        assert_eq!(lp_tokens_for(100, 700, 0, 7_000, 1_000), 0);
    }

    #[test]
    fn day_window_rolls_after_24_hours() {
        assert_eq!(roll_day(100, 50, 100 + 86_399), (100, 50));
        assert_eq!(roll_day(100, 50, 100 + 86_400), (86_500, 0));
    }
}
