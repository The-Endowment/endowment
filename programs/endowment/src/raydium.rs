//! Minimal, read-only views of Raydium CPMM accounts and the `swap_base_input`
//! CPI. Layouts and discriminators come from the on-chain IDL in
//! `idls/raydium_cp_swap.json` (raydium_cp_swap 0.2.0) and Raydium's source.
//!
//! `PoolState` and `ObservationState` are packed zero-copy accounts, so fields
//! are read at fixed byte offsets rather than through generated bindings, and
//! both accounts must have their exact expected length.

use anchor_lang::prelude::*;

use crate::{constants::TWAP_WINDOW_SECONDS, error::EndowmentError};

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
/// (Q32.32), over at least `TWAP_WINDOW_SECONDS` up to `now`.
///
/// Raydium records, on each swap, the price *before* that swap multiplied by the
/// seconds since the last record (`last_update_timestamp`). So:
/// - a swap earlier in the same transaction, or the same second, adds nothing
///   to the average: the time since the last record is zero;
/// - the stretch since the last record is weighted at the current spot price,
///   which has held since then (only swaps move a CPMM price, and every swap
///   updates the record).
///
/// Moving the average by x% therefore means holding the price x%·(window /
/// seconds held) away from fair value across real seconds, against arbitrage.
///
/// The start of the window is the newest observation at least one full window
/// old. Older observations may include up to Raydium's 15-second coalescing
/// interval beyond their timestamp; over a 10-minute window that misattributes
/// at most 2.5% of the window's weight, and only to prices that actually held.
pub fn twap_price_x32(
    observation: &[u8],
    pool: &Pubkey,
    token_index: usize,
    spot_price_x32: u128,
    now: u64,
) -> Result<u128> {
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
    let last_update = u64_at(observation, LAST_UPDATE_OFFSET);
    let (latest_ts, latest_cum) = cumulative(latest);
    // Legacy accounts may not have recorded `last_update_timestamp`.
    let last_update = if last_update == 0 { latest_ts } else { last_update };
    require!(last_update <= now, EndowmentError::InvalidPoolData);

    // Cumulative price up to now: the latest record, plus the current spot price
    // for the seconds since (zero if a swap already happened this second).
    let since_update = now - last_update;
    let cum_now = latest_cum.wrapping_add(spot_price_x32.wrapping_mul(since_update as u128));

    // The newest point at least a window old: the latest record itself (as of
    // `last_update`) if that's old enough, else walk back through the ring buffer.
    let cutoff = now.checked_sub(TWAP_WINDOW_SECONDS).ok_or(EndowmentError::TwapUnavailable)?;
    let (base_ts, base_cum) = if last_update <= cutoff {
        (last_update, latest_cum)
    } else {
        let mut found = None;
        for step in 1..OBSERVATION_NUM {
            let i = (latest + OBSERVATION_NUM - step) % OBSERVATION_NUM;
            let (ts, cum) = cumulative(i);
            if ts == 0 || ts > latest_ts {
                // Empty slot, or wrapped around to entries newer than `latest`: no older history.
                break;
            }
            if ts <= cutoff {
                found = Some((ts, cum));
                break;
            }
        }
        found.ok_or(EndowmentError::TwapUnavailable)?
    };

    let elapsed = now - base_ts;
    require!(elapsed >= TWAP_WINDOW_SECONDS, EndowmentError::TwapUnavailable);
    let twap = cum_now.wrapping_sub(base_cum) / elapsed as u128;
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

    const P: u128 = 7 << 32;

    #[test]
    fn twap_averages_recorded_prices_over_the_window() {
        let pool = Pubkey::new_unique();
        // Price 7 from t=1,000 to t=2,000, then 9 until the last update at t=2,500.
        let data = observation(&pool, 2, 2_500, &[(0, 1_000, 0), (1, 2_000, 7 * 1_000 << 32), (2, 2_000, (7 * 1_000 + 9 * 500) << 32)]);
        // At t=2,500, with a manipulated spot of 100 this very second: it gets no weight.
        let twap = twap_price_x32(&data, &pool, 0, 100 << 32, 2_500).unwrap();
        // Window base is the newest record ≤ 1,900: t=1,000. (7,000 + 4,500) / 1,500 = 7.67.
        assert_eq!(twap, ((7 * 1_000 + 9 * 500) << 32) / 1_500);
    }

    #[test]
    fn twap_weights_the_current_price_for_the_seconds_since_the_last_update() {
        let pool = Pubkey::new_unique();
        let data = observation(&pool, 0, 1_000, &[(0, 1_000, 0)]);
        // Nothing traded for 1,000 seconds: the average is the spot price that held.
        assert_eq!(twap_price_x32(&data, &pool, 0, P, 2_000).unwrap(), P);
    }

    #[test]
    fn twap_needs_a_full_window_of_history_and_the_right_pool() {
        let pool = Pubkey::new_unique();
        let data = observation(&pool, 0, 1_000, &[(0, 1_000, 0)]);
        assert!(twap_price_x32(&data, &pool, 0, P, 1_500).is_err());
        assert!(twap_price_x32(&data, &Pubkey::new_unique(), 0, P, 2_000).is_err());
        let mut short = data.clone();
        short.pop();
        assert!(twap_price_x32(&short, &pool, 0, P, 2_000).is_err());
    }
}
