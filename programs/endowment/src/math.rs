//! Buyback sizing and price protection.
//!
//! The floor is measured against the pool's time-weighted average price (TWAP),
//! not its spot price, so trades earlier in the same transaction or second
//! can't move it (see `raydium::twap_price_x32`). A buyback accepts no fill worse than
//!
//!   floor = amount_in × (1 − dividend_fee) × twap × (1 − coin_fee) × (1 − pool_fee − max_price_impact)
//!
//! where `twap` is the coin received per dividend, fees are the two mints'
//! Token-2022 transfer fees and Raydium's fee on input (all capped, see
//! `constants.rs`), and `max_price_impact` bounds how far this trade, plus any
//! drift of the spot price from the TWAP, may move the result.
//!
//! Each buy is sized to use at most half of the impact budget on its own price
//! impact (`amount_in ≤ reserve_in × max_price_impact / 2`), leaving the other
//! half for the spot price drifting from the TWAP.

const BPS: u128 = 10_000;
const Q32: u32 = 32;

/// Minimum acceptable coin received for `amount_in` dividend, against the TWAP.
pub fn min_acceptable_out(
    amount_in: u64,
    twap_price_x32: u128,
    pool_fee_bps: u64,
    dividend_fee_bps: u64,
    coin_fee_bps: u64,
    max_price_impact_bps: u64,
) -> Option<u64> {
    let net_in = amount_in as u128 * BPS.checked_sub(dividend_fee_bps as u128)? / BPS;
    let at_twap = net_in.checked_mul(twap_price_x32)? >> Q32;
    let after_coin_fee = at_twap * BPS.checked_sub(coin_fee_bps as u128)? / BPS;
    let allowance = BPS.checked_sub(pool_fee_bps as u128 + max_price_impact_bps as u128)?;
    u64::try_from(after_coin_fee * allowance / BPS).ok()
}

/// Coin per dividend at the given reserves, Q32.32.
pub fn spot_price_x32(reserve_dividend: u64, reserve_coin: u64) -> Option<u128> {
    if reserve_dividend == 0 {
        return None;
    }
    Some(((reserve_coin as u128) << Q32) / reserve_dividend as u128)
}

/// The largest buy whose own price impact stays within half the impact budget.
pub fn impact_cap(reserve_in: u64, max_price_impact_bps: u16) -> u64 {
    (reserve_in as u128 * max_price_impact_bps as u128 / (2 * BPS)) as u64
}

/// What a buyback may spend from `vault_balance`, leaving room for the tip and
/// donation (charged on top as `extra_bps` of the amount spent).
pub fn spendable(vault_balance: u64, extra_bps: u16) -> u64 {
    (vault_balance as u128 * BPS / (BPS + extra_bps as u128)) as u64
}

/// Token bucket: the allowance refills at `per_day` per 24 hours and holds at
/// most `capacity`, so no 24-hour window spends more than `per_day + capacity`.
pub fn refill(allowance: u64, updated_at: i64, now: i64, per_day: u64, capacity: u64) -> u64 {
    let elapsed = now.saturating_sub(updated_at).max(0) as u128;
    let refilled = allowance as u128 + elapsed * per_day as u128 / 86_400;
    refilled.min(capacity as u128) as u64
}

/// Splits a buyback after the milestone: `buy_bps` of it buys the coin; the rest
/// becomes liquidity. Returns (to_buy, to_liquidity).
pub fn split_buy(amount_in: u64, buy_bps: u16) -> (u64, u64) {
    let to_buy = (amount_in as u128 * buy_bps as u128 / BPS) as u64;
    (to_buy, amount_in - to_buy)
}

/// How much of `amount` dividend to swap so the rest pairs with what the swap
/// returns at the pool's ratio (a single-sided "zap"). Constant product with fee
/// `fee_bps` on input and reserve `reserve_in`:
///
///   s = (√(R²(2−f)² + 4(1−f)·a·R) − R(2−f)) / (2(1−f))
///
/// Falls back to half if the numbers would overflow.
pub fn zap_swap_amount(amount: u64, reserve_in: u64, fee_bps: u64) -> u64 {
    let half = amount / 2;
    if reserve_in == 0 || fee_bps >= BPS as u64 {
        return half;
    }
    let (a, r, f) = (amount as u128, reserve_in as u128, fee_bps as u128);
    let two_minus = 2 * BPS - f; // (2 − f) × BPS
    let one_minus = BPS - f; // (1 − f) × BPS
    let inner = r
        .checked_mul(r)
        .and_then(|r2| r2.checked_mul(two_minus * two_minus))
        .and_then(|lhs| {
            4u128
                .checked_mul(one_minus)
                .and_then(|x| x.checked_mul(a))
                .and_then(|x| x.checked_mul(r))
                .and_then(|x| x.checked_mul(BPS))
                .and_then(|rhs| lhs.checked_add(rhs))
        });
    let Some(inner) = inner else { return half };
    let root = isqrt(inner);
    let numerator = root.saturating_sub(r * two_minus);
    let s = numerator / (2 * one_minus);
    (s.min(a)) as u64
}

/// LP tokens to request so Raydium's rounded-up deposit fits within what we
/// have on both sides.
pub fn lp_tokens_for(dividend: u64, coin: u64, reserve_dividend: u64, reserve_coin: u64, lp_supply: u64) -> u64 {
    if reserve_dividend == 0 || reserve_coin == 0 || lp_supply == 0 {
        return 0;
    }
    let by_dividend = dividend as u128 * lp_supply as u128 / reserve_dividend as u128;
    let by_coin = coin as u128 * lp_supply as u128 / reserve_coin as u128;
    // One unit of headroom for Raydium's ceiling rounding.
    (by_dividend.min(by_coin).saturating_sub(1)).min(u64::MAX as u128) as u64
}

/// Integer square root (floor).
pub fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = 1u128 << ((128 - n.leading_zeros()).div_ceil(2));
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_is_measured_against_the_twap_and_applies_every_haircut() {
        // 1,000 in at 7 coin per dividend = 7,000 at the TWAP. 3% coin fee → 6,790.
        // 25 bps pool fee + 100 bps impact allowance → 6,790 × 0.9875 = 6,705.
        let twap = 7u128 << 32;
        assert_eq!(min_acceptable_out(1_000, twap, 25, 0, 300, 100), Some(6_705));
        // A 1% fee on the dividend going in: 990 × 7 = 6,930 → 6,722 → 6,637.
        assert_eq!(min_acceptable_out(1_000, twap, 25, 100, 300, 100), Some(6_637));
        assert_eq!(min_acceptable_out(1_000, twap, 9_950, 0, 0, 100), None);
    }

    #[test]
    fn spot_price_is_coin_per_dividend() {
        assert_eq!(spot_price_x32(10, 70), Some(7u128 << 32));
        assert_eq!(spot_price_x32(0, 70), None);
    }

    #[test]
    fn buys_use_half_the_impact_budget() {
        // 1% budget on a 10,000,000 reserve: at most 50,000 in.
        assert_eq!(impact_cap(10_000_000, 100), 50_000);
    }

    #[test]
    fn spendable_leaves_room_for_the_tip_and_donation() {
        assert_eq!(spendable(10_025, 25), 10_000);
        assert_eq!(spendable(10_055, 55), 10_000);
        assert_eq!(spendable(0, 25), 0);
    }

    #[test]
    fn allowance_refills_smoothly_and_caps() {
        // 86,400 per day refills 1 per second, up to the 1,000 capacity.
        assert_eq!(refill(0, 0, 500, 86_400, 1_000), 500);
        assert_eq!(refill(0, 0, 5_000, 86_400, 1_000), 1_000);
        assert_eq!(refill(700, 100, 100, 86_400, 1_000), 700);
    }

    #[test]
    fn split_sends_the_liquidity_share_aside() {
        assert_eq!(split_buy(1_000, 10_000), (1_000, 0));
        assert_eq!(split_buy(1_000, 7_000), (700, 300));
        assert_eq!(split_buy(1_001, 0), (0, 1_001));
    }

    #[test]
    fn zap_swaps_a_bit_less_than_half() {
        // With no fee, the zap swaps slightly less than half; with a fee, a bit more of the remainder is kept.
        let s0 = zap_swap_amount(1_000_000, 10_000_000_000, 0);
        let s1 = zap_swap_amount(1_000_000, 10_000_000_000, 125);
        assert!(s0 < 500_000 && s0 > 499_000, "{s0}");
        assert!(s1 > s0 && s1 < 510_000, "{s1}");
        // Degenerate inputs fall back to half.
        assert_eq!(zap_swap_amount(1_000, 0, 25), 500);
    }

    #[test]
    fn lp_request_fits_both_sides() {
        assert_eq!(lp_tokens_for(100, 700, 1_000, 7_000, 1_000), 99);
        assert_eq!(lp_tokens_for(100, 350, 1_000, 7_000, 1_000), 49);
        assert_eq!(lp_tokens_for(100, 700, 0, 7_000, 1_000), 0);
    }

    #[test]
    fn isqrt_is_exact_on_squares_and_floors_otherwise() {
        for n in [0u128, 1, 2, 3, 4, 15, 16, 17, 1 << 64, (1u128 << 100) + 12345] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{n}");
        }
    }
}
