//! Minimal, read-only views of Raydium CPMM accounts and the `swap_base_input`
//! CPI. Layouts and discriminators come from the on-chain IDL in
//! `idls/raydium_cp_swap.json` (raydium_cp_swap 0.2.0).
//!
//! `PoolState` is a packed zero-copy account, so fields are read at fixed
//! byte offsets rather than through generated bindings.

use anchor_lang::prelude::*;

use crate::error::EndowmentError;

pub const CPMM_PROGRAM_ID: Pubkey = pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
pub const CPMM_AUTH_SEED: &[u8] = b"vault_and_lp_mint_auth_seed";

pub const SWAP_BASE_INPUT_DISCRIMINATOR: [u8; 8] = [143, 190, 90, 218, 196, 30, 51, 222];
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
const AMM_CONFIG_DISCRIMINATOR: [u8; 8] = [218, 244, 33, 104, 203, 203, 43, 111];

/// Raydium fee rates are parts per million.
const FEE_RATE_DENOMINATOR: u64 = 1_000_000;

fn pubkey_at(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}

fn u64_at(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

pub struct PoolView {
    pub amm_config: Pubkey,
    pub vaults: [Pubkey; 2],
    pub mints: [Pubkey; 2],
    pub observation: Pubkey,
    /// Protocol + fund + creator fees held in each vault that are not
    /// swappable liquidity.
    pub reserved_fees: [u64; 2],
    pub creator_fee_enabled: bool,
}

impl PoolView {
    pub fn parse(data: &[u8]) -> Result<Self> {
        require!(
            data.len() >= 413 && data[..8] == POOL_STATE_DISCRIMINATOR,
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
    Ok((rate * 10_000).div_ceil(FEE_RATE_DENOMINATOR))
}
