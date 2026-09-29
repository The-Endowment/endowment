//! The daily commitment count, per endowment. No limit on landlords.
//!
//! Landlord sweeps run only while the endowment's landlords together hold
//! enough of its coin. Once a day anyone can start a count (`begin_count`),
//! count landlords in batches of any size across as many transactions as needed
//! (`count_landlords`), and finish it (`finish_count`) once every landlord
//! registered before it began has been counted, or after `COUNT_TIMEOUT_SECS`,
//! when anyone left uncounted counts zero.
//!
//! Each landlord counts for the smaller of its coin balance now and its balance
//! at its previous count (zero at its first), and only while its dividend
//! account still delegates to the endowment and it holds the minimum stake. So:
//! - coin must be held from one count to the next to count; coin moved into a
//!   landlord wallet (bought, borrowed or shuffled) adds nothing until the
//!   following count, and a drop counts at once;
//! - moving coin from one landlord wallet to another mid-count never makes it
//!   count twice in that count, because the receiving wallet can only be credited
//!   what it held at its previous count.
//!
//! Between counts, anyone can `refresh_landlords`: a decrease-only re-read that
//! lowers each landlord's recorded balance to what it holds now. Coin cycled
//! between two landlord wallets (so each holds it whenever it is counted) keeps
//! counting twice only if it also sits in each wallet at every refresh. The
//! automation refreshes every landlord at random times each day, reading many
//! landlords in one transaction, where the same coin can't be in two wallets at
//! once. What remains is an attacker who moves coin between wallets that happen
//! to be read in different transactions, paying the coin's transfer fee (3% for
//! $PENIS) on every move, with every move on-chain and every landlord's raw
//! balance published each round in `LandlordCounted`.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::{
    constants::*,
    error::EndowmentError,
    events::{CommitmentCounted, CountStarted, LandlordCounted},
    state::{Config, CountRound, Landlord},
    transfer::read_token_account,
};

#[derive(Accounts)]
pub struct BeginCount<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
}

pub fn handle_begin_count(ctx: Context<BeginCount>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let supply = ctx.accounts.coin_mint.supply;
    let config = &mut ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(!config.count.open, EndowmentError::CountOpen);
    require!(
        config.count.round == 0 || now.saturating_sub(config.count.started_at) >= COUNT_INTERVAL_SECS,
        EndowmentError::CountTooSoon
    );

    let round = config.count.round.checked_add(1).ok_or(EndowmentError::Overflow)?;
    config.count = CountRound {
        round,
        open: true,
        started_at: now,
        supply,
        expected: config.landlord_count,
        counted: 0,
        committed: 0,
    };
    emit!(CountStarted { config: config_key, round, expected: config.landlord_count, supply });
    Ok(())
}

/// Remaining accounts, three per landlord, any number of landlords, any order:
/// the landlord record (writable), its registered coin account and its
/// registered dividend account (either may be closed).
#[derive(Accounts)]
pub struct CountLandlords<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
}

pub fn handle_count_landlords<'info>(ctx: Context<'info, CountLandlords<'info>>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let program_id = ctx.program_id;
    let accounts = ctx.remaining_accounts;
    let config = &mut ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(config.count.open, EndowmentError::NoOpenCount);
    require!(!accounts.is_empty() && accounts.len() % 3 == 0, EndowmentError::InvalidCountAccount);

    let authority = Pubkey::create_program_address(
        &[AUTHORITY_SEED, config_key.as_ref(), &[config.authority_bump]],
        program_id,
    )
    .map_err(|_| error!(EndowmentError::InvalidCountAccount))?;
    let min_stake = config.min_stake(config.count.supply);
    let (round, coin_mint) = (config.count.round, config.coin_mint);

    for triple in accounts.chunks(3) {
        let (landlord_info, coin_info, dividend_info) = (&triple[0], &triple[1], &triple[2]);
        require!(landlord_info.is_writable, EndowmentError::InvalidCountAccount);
        let mut landlord: Account<Landlord> = Account::try_from(landlord_info)?;
        // This endowment's landlord, at its canonical address.
        require_keys_eq!(landlord.config, config_key, EndowmentError::InvalidCountAccount);
        let canonical = Pubkey::create_program_address(
            &[LANDLORD_SEED, config_key.as_ref(), landlord.owner.as_ref(), &[landlord.bump]],
            program_id,
        )
        .map_err(|_| error!(EndowmentError::InvalidCountAccount))?;
        require_keys_eq!(landlord_info.key(), canonical, EndowmentError::InvalidCountAccount);
        require_keys_eq!(coin_info.key(), landlord.coin_account, EndowmentError::InvalidCountAccount);
        require_keys_eq!(dividend_info.key(), landlord.dividend_account, EndowmentError::InvalidCountAccount);
        // Part of this round, and not counted in it yet.
        require!(config.expects(&landlord), EndowmentError::NotInCount);

        let balance = match read_token_account(coin_info)? {
            Some(coin) => {
                require_keys_eq!(coin.mint, coin_mint, EndowmentError::InvalidCountAccount);
                require_keys_eq!(coin.owner, landlord.owner, EndowmentError::InvalidCountAccount);
                coin.amount
            }
            None => 0,
        };
        let delegated = read_token_account(dividend_info)?
            .map(|d| d.owner == landlord.owner && d.delegates_to(&authority))
            .unwrap_or(false);

        let held = landlord.held(balance);
        let counted = if delegated && held >= min_stake { held } else { 0 };

        landlord.snapshot = balance;
        landlord.snapshot_valid = true;
        landlord.counted_round = round;
        landlord.counted_amount = counted;
        landlord.exit(program_id)?;

        config.count.counted = config.count.counted.checked_add(1).ok_or(EndowmentError::Overflow)?;
        config.count.committed = config.count.committed.checked_add(counted).ok_or(EndowmentError::Overflow)?;
        emit!(LandlordCounted {
            config: config_key,
            round,
            landlord: landlord_info.key(),
            owner: landlord.owner,
            counted,
            raw_balance: balance,
        });
    }
    Ok(())
}

/// Remaining accounts, two per landlord, any number, any order: the landlord
/// record (writable) and its registered coin account (may be closed).
#[derive(Accounts)]
pub struct RefreshLandlords<'info> {
    #[account(
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
}

/// Permissionless, decrease-only: lowers each landlord's recorded balance to what
/// it holds now, so the next count credits no more than that. It can never raise
/// anything, so calling it is always safe, as often as anyone likes.
pub fn handle_refresh_landlords<'info>(ctx: Context<'info, RefreshLandlords<'info>>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let program_id = ctx.program_id;
    let accounts = ctx.remaining_accounts;
    let coin_mint = ctx.accounts.config.coin_mint;
    require!(!accounts.is_empty() && accounts.len() % 2 == 0, EndowmentError::InvalidCountAccount);

    for pair in accounts.chunks(2) {
        let (landlord_info, coin_info) = (&pair[0], &pair[1]);
        require!(landlord_info.is_writable, EndowmentError::InvalidCountAccount);
        let mut landlord: Account<Landlord> = Account::try_from(landlord_info)?;
        require_keys_eq!(landlord.config, config_key, EndowmentError::InvalidCountAccount);
        require_keys_eq!(coin_info.key(), landlord.coin_account, EndowmentError::InvalidCountAccount);
        let balance = match read_token_account(coin_info)? {
            Some(coin) => {
                require_keys_eq!(coin.mint, coin_mint, EndowmentError::InvalidCountAccount);
                coin.amount
            }
            None => 0,
        };
        if landlord.snapshot_valid && balance < landlord.snapshot {
            landlord.snapshot = balance;
            landlord.exit(program_id)?;
        }
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
    pub config: Box<Account<'info, Config>>,
}

pub fn handle_finish_count(ctx: Context<FinishCount>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(config.count.open, EndowmentError::NoOpenCount);
    let complete = config.count.counted >= config.count.expected;
    let timed_out = now.saturating_sub(config.count.started_at) >= COUNT_TIMEOUT_SECS;
    require!(complete || timed_out, EndowmentError::CountIncomplete);

    let count = config.count;
    let committed_bps = if count.supply == 0 {
        0
    } else {
        ((count.committed as u128 * 10_000) / count.supply as u128).min(10_000) as u16
    };
    config.count.open = false;
    config.apply_committed_bps(committed_bps);
    config.last_count_at = now;
    config.last_count_bps = committed_bps;
    config.last_committed = count.committed;

    emit!(CommitmentCounted {
        config: config_key,
        round: count.round,
        expected: count.expected,
        counted: count.counted,
        committed: count.committed,
        committed_bps,
        active: config.active,
        timed_out: !complete,
    });
    Ok(())
}
