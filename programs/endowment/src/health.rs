//! Whether an endowment can trade at all right now: the checks a buyback needs
//! that don't depend on price or size. Sweeps run the same checks, so that when
//! buybacks can't run (a transfer hook switched on, a fee raised above the cap,
//! the pool's swaps disabled, a vault frozen) landlords' dividends stay with the
//! landlords instead of piling up in a vault that can't spend them.

use anchor_lang::prelude::*;

use crate::{
    constants::{MAX_POOL_FEE_BPS, MAX_TRANSFER_FEE_BPS},
    error::EndowmentError,
    raydium::{pool_fee_bps, PoolView},
    state::Config,
    transfer::{capped_transfer_fee_bps, hook_enabled},
};

/// Token account `state` byte: 2 = frozen (same offset in SPL Token and Token-2022).
const TOKEN_STATE_OFFSET: usize = 108;
const TOKEN_STATE_FROZEN: u8 = 2;

pub struct Tradeable {
    pub pool: PoolView,
    pub dividend_index: usize,
    pub coin_index: usize,
    /// All in basis points, all within their hard caps.
    pub pool_fee: u64,
    pub dividend_fee: u64,
    pub coin_fee: u64,
}

/// The accounts the checks read. `vaults` are token accounts that a buyback
/// moves tokens through: none may be frozen.
pub struct TradeAccounts<'a, 'info> {
    pub dividend_mint: &'a AccountInfo<'info>,
    pub coin_mint: &'a AccountInfo<'info>,
    pub pool_state: &'a AccountInfo<'info>,
    pub amm_config: &'a AccountInfo<'info>,
    pub pool_dividend_vault: &'a AccountInfo<'info>,
    pub pool_coin_vault: &'a AccountInfo<'info>,
    pub vaults: [&'a AccountInfo<'info>; 2],
}

pub fn ensure_tradeable(config: &Config, a: &TradeAccounts, epoch: u64) -> Result<Tradeable> {
    // Raydium can't pass transfer-hook accounts, so a hooked mint can't trade.
    require!(
        !hook_enabled(a.dividend_mint)? && !hook_enabled(a.coin_mint)?,
        EndowmentError::TransferHookEnabled
    );
    // Fees, all capped (current and scheduled): a raised fee halts trading
    // instead of lowering the floor or taxing what's swept.
    let dividend_fee = capped_transfer_fee_bps(a.dividend_mint, epoch)?;
    let coin_fee = capped_transfer_fee_bps(a.coin_mint, epoch)?;
    debug_assert!(dividend_fee <= MAX_TRANSFER_FEE_BPS && coin_fee <= MAX_TRANSFER_FEE_BPS);

    require_keys_eq!(a.pool_state.key(), config.pool, EndowmentError::WrongPool);
    let pool = PoolView::parse(&a.pool_state.try_borrow_data()?)?;
    let dividend_index = pool.index_of(&config.dividend_mint)?;
    let coin_index = pool.index_of(&config.coin_mint)?;
    require_keys_eq!(a.amm_config.key(), pool.amm_config, EndowmentError::WrongPool);
    require_keys_eq!(a.pool_dividend_vault.key(), pool.vaults[dividend_index], EndowmentError::WrongPool);
    require_keys_eq!(a.pool_coin_vault.key(), pool.vaults[coin_index], EndowmentError::WrongPool);
    require!(pool.swaps_enabled(), EndowmentError::PoolSwapDisabled);
    let pool_fee = pool_fee_bps(&a.amm_config.try_borrow_data()?, pool.creator_fee_enabled)?;
    require!(pool_fee <= MAX_POOL_FEE_BPS, EndowmentError::FeeTooHigh);

    for vault in [a.pool_dividend_vault, a.pool_coin_vault, a.vaults[0], a.vaults[1]] {
        require!(!is_frozen(vault)?, EndowmentError::VaultFrozen);
    }
    Ok(Tradeable { pool, dividend_index, coin_index, pool_fee, dividend_fee, coin_fee })
}

pub fn is_frozen(info: &AccountInfo) -> Result<bool> {
    let data = info.try_borrow_data()?;
    Ok(data.len() > TOKEN_STATE_OFFSET && data[TOKEN_STATE_OFFSET] == TOKEN_STATE_FROZEN)
}
