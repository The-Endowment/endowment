//! Minimal, read-only views of Raydium CPMM accounts and the `swap_base_input`
//! CPI. Layouts and discriminators come from the on-chain IDL in
//! `idls/raydium_cp_swap.json` (raydium_cp_swap 0.2.0) and Raydium's source.
//!
//! `PoolState` and `ObservationState` are packed zero-copy accounts, so fields
//! are read at fixed byte offsets rather than through generated bindings, and
//! both accounts must have their exact expected length.

use anchor_lang::prelude::*;

use crate::{
    constants::{
        MAX_STRETCH_SHARE_BPS, MIN_TWAP_WINDOW_SECONDS, RAYDIUM_OBSERVATION_COALESCE_SECONDS, TWAP_WINDOW_SECONDS,
    },
    error::EndowmentError,
};

pub const CPMM_PROGRAM_ID: Pubkey = pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
pub const CPMM_AUTH_SEED: &[u8] = b"vault_and_lp_mint_auth_seed";

pub const SWAP_BASE_INPUT_DISCRIMINATOR: [u8; 8] = [143, 190, 90, 218, 196, 30, 51, 222];
pub const DEPOSIT_DISCRIMINATOR: [u8; 8] = [242, 35, 198, 137, 82, 225, 242, 182];
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
const AMM_CONFIG_DISCRIMINATOR: [u8; 8] = [218, 244, 33, 104, 203, 203, 43, 111];
const OBSERVATION_DISCRIMINATOR: [u8; 8] = [122, 174, 197, 53, 129, 9, 165, 132];

const POOL_STATE_LEN: usize = 637;
const OBSERVATION_STATE_LEN: usize = 4_075;
const OBSERVATION_NUM: usize = 100;
/// discriminator (8) + initialized (1) + observation_index (2) + pool_id (32).
const OBSERVATIONS_OFFSET: usize = 43;
/// block_timestamp (8) + cumulative_token_0_price_x32 (16) + cumulative_token_1_price_x32 (16).
const OBSERVATION_LEN: usize = 40;
const LAST_UPDATE_OFFSET: usize = OBSERVATIONS_OFFSET + OBSERVATION_LEN * OBSERVATION_NUM;

/// Pool status bits: 1 = deposits disabled, 2 = withdrawals disabled, 4 = swaps disabled.
const STATUS_DEPOSIT_DISABLED: u8 = 1;
const STATUS_SWAP_DISABLED: u8 = 4;

/// Raydium fee rates are parts per million.
const FEE_RATE_DENOMINATOR: u64 = 1_000_000;

fn pubkey_at(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}

fn u64_at(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn u128_at(data: &[u8], offset: usize) -> u128 {
    u128::from_le_bytes(data[offset..offset + 16].try_into().unwrap())
}

pub struct PoolView {
    pub amm_config: Pubkey,
    pub vaults: [Pubkey; 2],
    pub mints: [Pubkey; 2],
    pub observation: Pubkey,
    pub lp_mint: Pubkey,
    pub status: u8,
    pub lp_supply: u64,
    /// Protocol + fund + creator fees held in each vault that are not
    /// swappable liquidity.
    pub reserved_fees: [u64; 2],
    pub creator_fee_enabled: bool,
}

impl PoolView {
    pub fn parse(data: &[u8]) -> Result<Self> {
        require!(
            data.len() == POOL_STATE_LEN && data[..8] == POOL_STATE_DISCRIMINATOR,
            EndowmentError::InvalidPoolData
        );
        let fees = |protocol: usize, fund: usize, creator: usize| {
            u64_at(data, protocol)
                .saturating_add(u64_at(data, fund))
                .saturating_add(u64_at(data, creator))
        };
        Ok(Self {
            amm_config: pubkey_at(data, 8),
            vaults: [pubkey_at(data, 72), pubkey_at(data, 104)],
            mints: [pubkey_at(data, 168), pubkey_at(data, 200)],
            observation: pubkey_at(data, 296),
            lp_mint: pubkey_at(data, 136),
            status: data[329],
            lp_supply: u64_at(data, 333),
            reserved_fees: [fees(341, 357, 397), fees(349, 365, 405)],
            creator_fee_enabled: data[390] != 0,
        })
    }

    /// Index (0 or 1) of `mint` in the pool.
    pub fn index_of(&self, mint: &Pubkey) -> Result<usize> {
        self.mints
            .iter()
            .position(|m| m == mint)
            .ok_or_else(|| error!(EndowmentError::WrongPool))
    }

    pub fn swaps_enabled(&self) -> bool {
        self.status & STATUS_SWAP_DISABLED == 0
    }

    pub fn deposits_enabled(&self) -> bool {
        self.status & STATUS_DEPOSIT_DISABLED == 0
    }

    /// Swappable reserve in vault `index`: its balance minus fees held for others.
    /// Fails closed if the pool claims more fees than the vault holds.
    pub fn reserve(&self, index: usize, vault_balance: u64) -> Result<u64> {
        let fees = self.reserved_fees[index];
        require!(fees <= vault_balance, EndowmentError::InvalidPoolData);
        let reserve = vault_balance - fees;
        require!(reserve > 0, EndowmentError::InvalidPoolData);
        Ok(reserve)
    }
}

/// The pool's total fee on input, rounded up to whole basis points.
pub fn pool_fee_bps(amm_config: &[u8], creator_fee_enabled: bool) -> Result<u64> {
    require!(
        amm_config.len() >= 116 && amm_config[..8] == AMM_CONFIG_DISCRIMINATOR,
        EndowmentError::InvalidPoolData
    );
    let trade_fee_rate = u64_at(amm_config, 12);
    let creator_fee_rate = if creator_fee_enabled { u64_at(amm_config, 108) } else { 0 };
    let rate = trade_fee_rate.saturating_add(creator_fee_rate);
    Ok((rate.saturating_mul(10_000)).div_ceil(FEE_RATE_DENOMINATOR))
}

/// Time-weighted average price of `token_index` in units of the other token
/// (Q32.32), from the pool's recorded price history only.
///
/// How Raydium records prices: each observation holds a timestamp and a
/// cumulative price. A swap at least 15 seconds after the latest observation's
/// timestamp opens a new one; a swap sooner than that adds to the latest one's
/// cumulative (and `last_update_timestamp`) without moving its timestamp. Each
/// addition is the price *before* that swap times the seconds since the last
/// addition. So:
/// - a swap earlier in the same transaction, or the same second, adds nothing;
/// - a stretch between records is priced at the moment it closes, so a token
///   transfer straight into a pool vault (which moves the price but records
///   nothing) just before a swap colours the whole stretch before it.
///
/// Therefore:
/// - the average ends at the latest record (`last_update_timestamp`, when its
///   cumulative is exact). Nothing after it is extrapolated, so moving the spot
///   price without a swap moves the average not at all (the spot band in
///   `buyback` then refuses the buy);
/// - each older record's cumulative runs somewhere between 0 and
///   `RAYDIUM_OBSERVATION_COALESCE_SECONDS` (14 s) past its timestamp. Taking
///   the timestamp itself (as round 1 did) biased the average low by up to
///   14 s / span whenever swaps had coalesced into the starting record, which an
///   attacker can arrange (F-01). Records are taken as of the middle of that
///   range instead: no bias either way, and at most ±7 s / span of error at the
///   start (±0.78% over the shortest window), which the floor allows for
///   (`TWAP_READER_ERROR_BPS`);
/// - it walks back through the records until it has `TWAP_WINDOW_SECONDS` of
///   weight, where no single stretch weighs more than `MAX_STRETCH_SHARE_BPS` of
///   the window: a longer stretch counts at its own average price, for that
///   capped time. A long quiet stretch (whose price is the pool's price now)
///   needs no more history than any other to average over;
/// - if the records run out first (a busy pool fills Raydium's 100-record ring
///   in as little as 99 × 15 s = 1,485 s), it averages over what they hold, as
///   long as that is at least `MIN_TWAP_WINDOW_SECONDS` and no one stretch is
///   more than `MAX_STRETCH_SHARE_BPS` of it. Either way one coloured stretch
///   is at most a quarter of the average, so the spot band catches it.
pub fn twap_price_x32(observation: &[u8], pool: &Pubkey, token_index: usize, now: u64) -> Result<u128> {
    require!(
        observation.len() == OBSERVATION_STATE_LEN && observation[..8] == OBSERVATION_DISCRIMINATOR,
        EndowmentError::InvalidPoolData
    );
    require!(observation[8] != 0, EndowmentError::TwapUnavailable);
    require_keys_eq!(pubkey_at(observation, 11), *pool, EndowmentError::WrongPool);
    let latest = u16::from_le_bytes([observation[9], observation[10]]) as usize;
    require!(latest < OBSERVATION_NUM, EndowmentError::InvalidPoolData);

    let cumulative = |i: usize| {
        let at = OBSERVATIONS_OFFSET + OBSERVATION_LEN * i;
        (u64_at(observation, at), u128_at(observation, at + 8 + 16 * token_index))
    };
    let (latest_ts, latest_cum) = cumulative(latest);
    let last_update = u64_at(observation, LAST_UPDATE_OFFSET);
    // Legacy accounts may not have recorded `last_update_timestamp`.
    let end = if last_update == 0 { latest_ts } else { last_update };
    require!(end >= latest_ts && end <= now, EndowmentError::InvalidPoolData);

    let cap = TWAP_WINDOW_SECONDS * MAX_STRETCH_SHARE_BPS / 10_000;
    // The newer end of the stretch being added: its timestamp, cumulative time
    // (the latest record is exact as of `end`), and weighted price and weight so far.
    let (mut newer_ts, mut newer_time, mut newer_cum) = (latest_ts, end, latest_cum);
    let (mut weighted, mut weight, mut longest) = (0u128, 0u64, 0u64);
    for step in 1..OBSERVATION_NUM {
        let i = (latest + OBSERVATION_NUM - step) % OBSERVATION_NUM;
        let (ts, cum) = cumulative(i);
        if ts == 0 || ts >= newer_ts {
            // Empty slot, or wrapped around to newer entries: no older history.
            break;
        }
        let time = (ts + RAYDIUM_OBSERVATION_COALESCE_SECONDS / 2).min(newer_time);
        let seconds = newer_time - time;
        let delta = newer_cum.wrapping_sub(cum);
        let (add, secs) = if seconds > cap {
            // Its average price, for the capped time.
            ((delta / seconds as u128).checked_mul(cap as u128).ok_or(EndowmentError::TwapUnavailable)?, cap)
        } else {
            (delta, seconds)
        };
        weighted = weighted.checked_add(add).ok_or(EndowmentError::TwapUnavailable)?;
        weight += secs;
        longest = longest.max(secs);
        if weight >= TWAP_WINDOW_SECONDS {
            break;
        }
        (newer_ts, newer_time, newer_cum) = (ts, time, cum);
    }
    // The full window, or (the records having run out) a shorter one of at
    // least the minimum in which no stretch outweighs its share.
    require!(
        weight >= TWAP_WINDOW_SECONDS
            || (weight >= MIN_TWAP_WINDOW_SECONDS
                && (longest as u128) * 10_000 <= (weight as u128) * MAX_STRETCH_SHARE_BPS as u128),
        EndowmentError::TwapUnavailable
    );
    let twap = weighted / weight as u128;
    require!(twap > 0, EndowmentError::TwapUnavailable);
    Ok(twap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(pool: &Pubkey, latest: u16, last_update: u64, entries: &[(usize, u64, u128)]) -> Vec<u8> {
        let mut data = vec![0u8; OBSERVATION_STATE_LEN];
        data[..8].copy_from_slice(&OBSERVATION_DISCRIMINATOR);
        data[8] = 1;
        data[9..11].copy_from_slice(&latest.to_le_bytes());
        data[11..43].copy_from_slice(pool.as_ref());
        for &(i, ts, cum) in entries {
            let at = OBSERVATIONS_OFFSET + OBSERVATION_LEN * i;
            data[at..at + 8].copy_from_slice(&ts.to_le_bytes());
            // Token 0's cumulative price.
            data[at + 8..at + 24].copy_from_slice(&cum.to_le_bytes());
        }
        data[LAST_UPDATE_OFFSET..LAST_UPDATE_OFFSET + 8].copy_from_slice(&last_update.to_le_bytes());
        data
    }

    /// Records every `gap` seconds from `from`, at a constant price per record
    /// (the price of the stretch each record closes), each cumulative exact as
    /// of `coalesced` seconds after its timestamp.
    fn history(prices: &[u128], from: u64, gap: u64, coalesced: u64) -> (Vec<(usize, u64, u128)>, u64) {
        let mut entries = vec![(0, from, prices[0] * coalesced as u128)];
        let mut cum = prices[0] * coalesced as u128;
        let mut exact = from + coalesced;
        for (i, &p) in prices.iter().enumerate().skip(1) {
            let ts = from + gap * i as u64;
            cum += p * (ts + coalesced - exact) as u128;
            exact = ts + coalesced;
            entries.push((i, ts, cum));
        }
        (entries, exact)
    }

    const P: u128 = 7 << 32;

    #[test]
    fn twap_averages_recorded_prices_and_ignores_the_spot_price() {
        let pool = Pubkey::new_unique();
        // Price 7 every 100 s from t=1,000 to 3,800, then a stretch closed at 9.
        let mut prices = vec![P; 29];
        prices.push(9 << 32);
        let (entries, end) = history(&prices, 1_000, 100, 0);
        let data = observation(&pool, 29, end, &entries);
        // Long after, whatever the pool's price now: only the record counts.
        let twap = twap_price_x32(&data, &pool, 0, end + 100_000).unwrap();
        // Span ends at 3,900; the newest start at least 1,800 back is the record
        // at 2,000, taken as of 2,007: (18 × 100 × 7 + 100 × 9) / 1,893.
        assert_eq!(twap, ((1_800 * 7 + 900) << 32) / 1_893);
    }

    #[test]
    fn f01_a_coalesced_base_record_no_longer_biases_the_twap_low() {
        let pool = Pubkey::new_unique();
        let bound = P * 7 / TWAP_WINDOW_SECONDS as u128;
        // Constant price, every record's cumulative running 14 s past its timestamp
        // (the attacker's case: round 1 read this 14 s / span low).
        let (entries, end) = history(&[P; 40], 1_000, 60, 14);
        let data = observation(&pool, 39, end, &entries);
        let twap = twap_price_x32(&data, &pool, 0, end).unwrap();
        assert!(P - twap <= bound, "{twap}");
        // Nothing coalesced: it reads at most as far high.
        let (entries, end) = history(&[P; 40], 1_000, 60, 0);
        let data = observation(&pool, 39, end, &entries);
        let twap = twap_price_x32(&data, &pool, 0, end).unwrap();
        assert!(twap >= P && twap - P <= bound, "{twap}");
    }

    #[test]
    fn a_long_quiet_stretch_needs_no_more_history_than_the_window() {
        let pool = Pubkey::new_unique();
        // A day with no swap, then one: the day counts for a quarter of the
        // window, at its own price, and 22.5 minutes of earlier history fill the rest.
        let (mut entries, _) = history(&[P; 20], 1_000, 100, 0);
        let day_end = 1_000 + 19 * 100 + 86_400;
        entries.push((20, day_end, entries[19].2 + 8 * (1 << 32) * 86_400));
        let data = observation(&pool, 20, day_end, &entries);
        let twap = twap_price_x32(&data, &pool, 0, day_end).unwrap();
        // (8 + 3 × 7) / 4 = 7.25
        assert!(twap > (29 << 32) / 4 - P / 100 && twap < (29 << 32) / 4 + P / 100, "{twap}");
    }

    #[test]
    fn r2cc01_one_long_stretch_closed_at_a_moved_price_is_at_most_a_quarter_of_the_weight() {
        let pool = Pubkey::new_unique();
        // A quiet pool: records every 100 s at 7, then an hour with no swap, then
        // coin sent straight into the pool and a swap that prices the hour at 14,
        // and a second swap 15 s later.
        let (mut entries, _) = history(&[P; 90], 1_000, 100, 0);
        let hour_end = 1_000 + 89 * 100 + 3_600;
        let mut cum = entries[89].2 + 2 * P * 3_600;
        entries.push((90, hour_end, cum));
        cum += 2 * P * 15;
        entries.push((91, hour_end + 15, cum));
        let data = observation(&pool, 91, hour_end + 15, &entries);
        let twap = twap_price_x32(&data, &pool, 0, hour_end + 15).unwrap();
        // The hour counts for at most a quarter of the 30-minute window:
        // ≤ (3 × 7 + 14) / 4 = 8.75.
        assert!(twap <= P * 5 / 4 + P / 50, "{twap}");
        assert!(twap > P * 5 / 4 - P / 50, "{twap}");
    }

    #[test]
    fn twap_needs_enough_history_and_the_right_pool() {
        let pool = Pubkey::new_unique();
        // 900 s of records (893 s of weight, after the reader's 7 s) is not a window.
        let (entries, end) = history(&[P; 10], 1_000, 100, 0);
        let data = observation(&pool, 9, end, &entries);
        assert!(twap_price_x32(&data, &pool, 0, end).is_err());
        // 1,000 s is.
        let (entries, end) = history(&[P; 11], 1_000, 100, 0);
        let data = observation(&pool, 10, end, &entries);
        assert!(twap_price_x32(&data, &pool, 0, end).is_ok());
        let (entries, end) = history(&[P; 30], 1_000, 100, 0);
        let data = observation(&pool, 29, end, &entries);
        assert!(twap_price_x32(&data, &pool, 0, end).is_ok());
        assert!(twap_price_x32(&data, &pool, 0, end - 1).is_err(), "a record from the future");
        assert!(twap_price_x32(&data, &Pubkey::new_unique(), 0, end).is_err());
        let mut short = data.clone();
        short.pop();
        assert!(twap_price_x32(&short, &pool, 0, end).is_err());
        // One record, however old, spans nothing.
        let data = observation(&pool, 0, 1_000, &[(0, 1_000, 0)]);
        assert!(twap_price_x32(&data, &pool, 0, 100_000).is_err());
    }

    /// Fills the whole ring with records `gap` seconds apart, the latest at
    /// index `latest` (so it wraps), at a constant price.
    fn full_ring(pool: &Pubkey, gap: u64, latest: usize) -> (Vec<u8>, u64) {
        let (history, end) = history(&[P; OBSERVATION_NUM], 1_000, gap, 0);
        let entries: Vec<_> = history
            .into_iter()
            .map(|(i, ts, cum)| ((latest + 1 + i) % OBSERVATION_NUM, ts, cum))
            .collect();
        (observation(pool, latest as u16, end, &entries), end)
    }

    #[test]
    fn r3tw01_a_ring_full_of_records_15_seconds_apart_still_gives_a_twap() {
        let pool = Pubkey::new_unique();
        // A busy pool (or a flood of dust swaps): 100 records every 15 s span
        // 1,485 s, less than the full window. The TWAP averages over all of it.
        for latest in [99, 0, 37] {
            let (data, end) = full_ring(&pool, 15, latest);
            let twap = twap_price_x32(&data, &pool, 0, end).unwrap();
            assert!(twap.abs_diff(P) <= P * TWAP_READER_ERROR / 10_000, "{latest}: {twap}");
        }
    }

    const TWAP_READER_ERROR: u128 = crate::constants::TWAP_READER_ERROR_BPS as u128;

    #[test]
    fn r3tw01_a_short_ring_is_refused_below_the_minimum_window() {
        let pool = Pubkey::new_unique();
        // 100 records 9 s apart (not possible on Raydium, which spaces them at
        // least 15 s) would span 891 s: below the minimum, refused.
        let (data, end) = full_ring(&pool, 9, 99);
        assert!(twap_price_x32(&data, &pool, 0, end).is_err());
        let (data, end) = full_ring(&pool, 10, 99);
        assert!(twap_price_x32(&data, &pool, 0, end).is_ok());
    }

    #[test]
    fn r3tw04_one_stretch_is_at_most_a_quarter_of_a_short_window() {
        let pool = Pubkey::new_unique();
        // A young pool: ten minutes of records, then 450 s closed at 3× the
        // price. 1,050 s of weight, but one stretch is 43% of it: refused
        // until more history accrues.
        let (mut entries, _) = history(&[P; 7], 1_000, 100, 0);
        let end = 1_000 + 6 * 100 + 450;
        entries.push((7, end, entries[6].2 + 3 * P * 450));
        let data = observation(&pool, 7, end, &entries);
        assert!(twap_price_x32(&data, &pool, 0, end).is_err());
    }
}
