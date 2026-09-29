//! The daily commitment count, per endowment.
//!
//! Landlord sweeps run only while the endowment's landlords together hold
//! enough of its coin. Anyone can run the count, at most once a day, and it
//! reads **every landlord in one instruction**, so the same coin can never be
//! counted twice by moving it between landlords mid-count.
//!
//! Each landlord counts for the smaller of its coin balance now and its balance
//! at the previous count (and nothing at its first count), so coin must be held
//! across a full count interval to count: borrowing or buying coin just before a
//! count adds nothing. A landlord counts only while its dividend account still
//! delegates to the endowment and it holds the minimum stake.
//!
//! The count then switches sweeps on at `activate_bps` of supply and off below
//! `deactivate_bps`, unchanged in between.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::{
    constants::*,
    error::EndowmentError,
    events::CommitmentCounted,
    state::{Config, Roster},
    transfer::read_token_account,
};

/// Remaining accounts: for each roster entry, in roster order, its registered
/// coin account and then its registered dividend account (either may be closed).
#[derive(Accounts)]
pub struct CountCommitment<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [ROSTER_SEED, config.key().as_ref()], bump = config.roster_bump)]
    pub roster: Box<Account<'info, Roster>>,
    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
}

pub fn handle_count_commitment<'info>(ctx: Context<'info, CountCommitment<'info>>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(
        config.last_count_at == 0 || now.saturating_sub(config.last_count_at) >= COUNT_INTERVAL_SECS,
        EndowmentError::CountTooSoon
    );
    let authority = Pubkey::create_program_address(
        &[AUTHORITY_SEED, config_key.as_ref(), &[config.authority_bump]],
        ctx.program_id,
    )
    .map_err(|_| error!(EndowmentError::InvalidCountAccount))?;
    let supply = ctx.accounts.coin_mint.supply;
    let min_stake = config.min_stake(supply);
    let coin_mint = config.coin_mint;

    let accounts = ctx.remaining_accounts;
    let roster = &mut ctx.accounts.roster;
    // Every landlord, exactly once, in roster order.
    require!(accounts.len() == 2 * roster.entries.len(), EndowmentError::InvalidCountAccount);

    let mut committed: u64 = 0;
    for (entry, pair) in roster.entries.iter_mut().zip(accounts.chunks(2)) {
        let (coin_info, dividend_info) = (&pair[0], &pair[1]);
        require_keys_eq!(coin_info.key(), entry.coin_account, EndowmentError::InvalidCountAccount);
        require_keys_eq!(dividend_info.key(), entry.dividend_account, EndowmentError::InvalidCountAccount);

        let balance = match read_token_account(coin_info)? {
            Some(coin) => {
                require_keys_eq!(coin.mint, coin_mint, EndowmentError::InvalidCountAccount);
                require_keys_eq!(coin.owner, entry.owner, EndowmentError::InvalidCountAccount);
                coin.amount
            }
            None => 0,
        };
        let delegated = read_token_account(dividend_info)?
            .map(|d| d.owner == entry.owner && d.delegates_to(&authority))
            .unwrap_or(false);

        let held = if entry.snapshot_valid { balance.min(entry.snapshot) } else { 0 };
        let counted = if delegated && held >= min_stake { held } else { 0 };
        committed = committed.checked_add(counted).ok_or(EndowmentError::Overflow)?;

        entry.snapshot = balance;
        entry.snapshot_valid = true;
    }

    let committed_bps =
        if supply == 0 { 0 } else { ((committed as u128 * 10_000) / supply as u128).min(10_000) as u16 };
    let landlords = roster.entries.len() as u32;
    let config = &mut ctx.accounts.config;
    config.apply_committed_bps(committed_bps);
    config.last_count_at = now;
    config.last_count_bps = committed_bps;
    config.last_committed = committed;

    emit!(CommitmentCounted { config: config_key, landlords, committed, committed_bps, active: config.active });
    Ok(())
}
