//! The daily commitment count, per endowment.
//!
//! Landlord sweeps run only while the endowment's registered landlords together
//! hold enough of its coin. Anyone can run the count, at most once a day:
//!
//! 1. `begin_count` opens a round and fixes how many landlords it expects.
//! 2. `count_landlords` (any number of calls) reads each landlord's coin
//!    balance straight from its registered token account.
//! 3. `finish_count` succeeds only once every expected landlord is counted,
//!    then switches sweeps on at `activate_bps` of supply and off below
//!    `deactivate_bps`.
//!
//! A landlord can be counted at most once per round, and only in its own
//! endowment's count. Landlords who register mid-round sit it out; landlords
//! who leave mid-round are removed from it. A round can therefore always be
//! finished, and nothing is counted twice.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount};

use crate::{
    constants::*,
    error::EndowmentError,
    events::{CountFinished, CountStarted},
    state::{Config, Landlord},
};

#[derive(Accounts)]
pub struct BeginCount<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,
}

pub fn handle_begin_count(ctx: Context<BeginCount>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    let landlord_count = config.landlord_count;
    let count = &mut config.count;
    require!(!count.open, EndowmentError::CountOpen);
    require!(
        count.round == 0 || now.saturating_sub(count.started_at) >= COUNT_INTERVAL_SECS,
        EndowmentError::CountTooSoon
    );
    count.round = count.round.checked_add(1).ok_or(EndowmentError::Overflow)?;
    count.open = true;
    count.started_at = now;
    count.expected = landlord_count;
    count.counted = 0;
    count.committed = 0;
    emit!(CountStarted { config: config_key, round: count.round, expected: count.expected });
    Ok(())
}

/// Remaining accounts: pairs of `[landlord PDA (writable), its registered coin account]`.
#[derive(Accounts)]
pub struct CountLandlords<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,
}

pub fn handle_count_landlords<'info>(ctx: Context<'info, CountLandlords<'info>>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let coin_mint = ctx.accounts.config.coin_mint;
    let count = &mut ctx.accounts.config.count;
    require!(count.open, EndowmentError::CountNotOpen);
    let pairs = ctx.remaining_accounts;
    require!(!pairs.is_empty() && pairs.len() % 2 == 0, EndowmentError::InvalidCountAccount);

    for pair in pairs.chunks(2) {
        let (landlord_info, coin_info) = (&pair[0], &pair[1]);
        require!(landlord_info.is_writable, EndowmentError::InvalidCountAccount);
        // Owner and discriminator are checked; only this program creates Landlords.
        let mut landlord: Account<Landlord> = Account::try_from(landlord_info)?;
        // Only this endowment's landlords, at their canonical address.
        require_keys_eq!(landlord.config, config_key, EndowmentError::InvalidCountAccount);
        let expected = Pubkey::create_program_address(
            &[LANDLORD_SEED, config_key.as_ref(), landlord.owner.as_ref(), &[landlord.bump]],
            ctx.program_id,
        )
        .map_err(|_| error!(EndowmentError::InvalidCountAccount))?;
        require_keys_eq!(landlord_info.key(), expected, EndowmentError::InvalidCountAccount);
        require!(landlord.counted_round < count.round, EndowmentError::InvalidCountAccount);
        require_keys_eq!(coin_info.key(), landlord.coin_account, EndowmentError::InvalidCountAccount);

        // A closed coin account holds nothing; any live one must be the
        // landlord's own account for this endowment's coin.
        let balance = if coin_info.data_is_empty() {
            0
        } else {
            let account: InterfaceAccount<TokenAccount> = InterfaceAccount::try_from(coin_info)
                .map_err(|_| error!(EndowmentError::InvalidCountAccount))?;
            require_keys_eq!(account.mint, coin_mint, EndowmentError::InvalidCountAccount);
            require_keys_eq!(account.owner, landlord.owner, EndowmentError::InvalidCountAccount);
            account.amount
        };

        landlord.counted_round = count.round;
        landlord.counted_balance = balance;
        landlord.exit(ctx.program_id)?;

        count.committed = count.committed.checked_add(balance).ok_or(EndowmentError::Overflow)?;
        count.counted = count.counted.checked_add(1).ok_or(EndowmentError::Overflow)?;
    }
    Ok(())
}

#[derive(Accounts)]
pub struct FinishCount<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,
    #[account(address = config.coin_mint)]
    pub coin_mint: InterfaceAccount<'info, Mint>,
}

pub fn handle_finish_count(ctx: Context<FinishCount>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let supply = ctx.accounts.coin_mint.supply;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(config.count.open, EndowmentError::CountNotOpen);
    require!(config.count.counted >= config.count.expected, EndowmentError::CountIncomplete);

    let committed_bps = if supply == 0 {
        0
    } else {
        ((config.count.committed as u128 * 10_000) / supply as u128).min(10_000) as u16
    };
    config.apply_committed_bps(committed_bps);
    config.count.open = false;
    config.count.last_committed_bps = committed_bps;
    config.count.last_finished_at = now;

    emit!(CountFinished {
        config: config_key,
        round: config.count.round,
        committed: config.count.committed,
        committed_bps,
        active: config.active,
    });
    Ok(())
}
