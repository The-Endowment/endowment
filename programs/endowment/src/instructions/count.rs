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
//! at its previous read (zero at its first), and only while its dividend
//! account still delegates to the endowment and it holds the round's minimum
//! stake. So coin must be held from one count to the next to count; coin moved
//! into a landlord wallet adds nothing until the following count, and a drop
//! counts at once.
//!
//! Anti-shuffle: the refresher attestation. On its own, the rule above lets one
//! holding be counted in several wallets: count A, move the coin to B, count B,
//! move it back before A's next read, so each wallet shows it at each of its own
//! reads. Each endowment therefore names a `refresher` (a timelocked parameter).
//! Between counts the refresher re-reads every landlord (`refresh_landlords`),
//! at times the landlords don't choose, and a landlord only counts once it has
//! been so read `REQUIRED_ATTESTATIONS` times since its last count read, each
//! read at least `MIN_ATTEST_SPACING_SECS` after the one before:
//! - every read is decrease-only: the landlord's recorded balance drops to what
//!   it holds at that moment, and a landlord found not delegated loses its
//!   record and its reads (so approving just for the count, then revoking,
//!   counts nothing);
//! - coin can be in only one wallet at the moment a read lands, so to count a
//!   holding in two wallets it must be moved into each wallet ahead of each of
//!   that wallet's reads, several reads in a row, each at a time the attacker
//!   doesn't choose. The keeper sends every transaction of a pass at once and
//!   runs several independently shuffled passes a day, so there is little
//!   between one read and the next to react to; every hop pays the coin's
//!   transfer fee (if any) and shows on-chain;
//! - a landlord still short of its reads when it would be counted is left
//!   pending, not counted as zero: the refresher can read it and it can be
//!   counted later in the same round (until the round times out), so starting
//!   a count early can't zero anyone. A count can only start once the refresher
//!   has read someone since the last one began;
//! - with no refresher set (or one that never runs) nobody counts, so the
//!   endowment can't switch on: the defence fails safe.
//!
//! The refresher is trusted. It can't move anything, but it chooses when it
//! reads: one that colludes with a landlord can time its reads to whenever the
//! landlord's coin sits in each of its wallets and so have it counted several
//! times, and it can leave landlords out by not reading them. Every read is
//! public (`LandlordsAttested` lists the landlords read), the refresher is shown
//! on each endowment's page, and it can resign (`resign_refresher`) at any time,
//! which switches sweeps off at once and voids every read it made (reads are
//! tagged with the endowment's `refresher_epoch`).

use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::{
    constants::*,
    error::EndowmentError,
    events::{CommitmentCounted, CountStarted, LandlordCounted, LandlordsAttested},
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
    // Only once the refresher has read someone since the last count began, so
    // nobody can open (and time out) rounds the refresher hasn't reached.
    require!(
        config.count.round == 0 || config.landlord_count == 0 || config.last_attested_at > config.count.started_at,
        EndowmentError::NotAttested
    );

    let round = config.count.round.checked_add(1).ok_or(EndowmentError::Overflow)?;
    let min_stake = config.min_stake(supply);
    config.count = CountRound {
        round,
        open: true,
        started_at: now,
        supply,
        expected: config.landlord_count,
        counted: 0,
        committed: 0,
        min_stake,
    };
    emit!(CountStarted { config: config_key, round, expected: config.landlord_count, supply });
    Ok(())
}

/// Remaining accounts, three per landlord, any number of landlords, any order:
/// the landlord record (writable), its registered coin account and its
/// registered dividend account (either may be closed). A landlord record that
/// was closed since the batch was built is skipped.
#[derive(Accounts)]
pub struct CountLandlords<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
}

/// A landlord's coin balance and whether its dividend account delegates to the
/// endowment. A coin account that isn't the owner's any more (legacy SPL Token
/// lets an owner reassign it) holds nothing for this landlord.
fn read_landlord(
    landlord: &Landlord,
    coin_info: &AccountInfo,
    dividend_info: &AccountInfo,
    coin_mint: &Pubkey,
    authority: &Pubkey,
) -> Result<(u64, bool)> {
    let balance = match read_token_account(coin_info)? {
        Some(coin) => {
            require_keys_eq!(coin.mint, *coin_mint, EndowmentError::InvalidCountAccount);
            if coin.owner == landlord.owner {
                coin.amount
            } else {
                0
            }
        }
        None => 0,
    };
    let delegated = read_token_account(dividend_info)?
        .map(|d| d.owner == landlord.owner && d.delegates_to(authority))
        .unwrap_or(false);
    Ok((balance, delegated))
}

/// This endowment's landlord at `info`, at its canonical address, with its
/// registered accounts; `None` if the record has been closed.
fn load_landlord<'info>(
    info: &'info AccountInfo<'info>,
    coin_info: &AccountInfo,
    dividend_info: &AccountInfo,
    config_key: &Pubkey,
    program_id: &Pubkey,
) -> Result<Option<Account<'info, Landlord>>> {
    require!(info.is_writable, EndowmentError::InvalidCountAccount);
    if info.data_is_empty() && *info.owner == anchor_lang::system_program::ID {
        return Ok(None);
    }
    let landlord: Account<Landlord> = Account::try_from(info)?;
    require_keys_eq!(landlord.config, *config_key, EndowmentError::InvalidCountAccount);
    let canonical = Pubkey::create_program_address(
        &[LANDLORD_SEED, config_key.as_ref(), landlord.owner.as_ref(), &[landlord.bump]],
        program_id,
    )
    .map_err(|_| error!(EndowmentError::InvalidCountAccount))?;
    require_keys_eq!(info.key(), canonical, EndowmentError::InvalidCountAccount);
    require_keys_eq!(coin_info.key(), landlord.coin_account, EndowmentError::InvalidCountAccount);
    require_keys_eq!(dividend_info.key(), landlord.dividend_account, EndowmentError::InvalidCountAccount);
    Ok(Some(landlord))
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
    let (round, coin_mint, min_stake) = (config.count.round, config.coin_mint, config.count.min_stake);

    for triple in accounts.chunks(3) {
        let (landlord_info, coin_info, dividend_info) = (&triple[0], &triple[1], &triple[2]);
        let Some(mut landlord) = load_landlord(landlord_info, coin_info, dividend_info, &config_key, program_id)?
        else {
            continue;
        };
        // Part of this round, and not counted in it yet.
        require!(config.expects(&landlord), EndowmentError::NotInCount);

        let (balance, delegated) = read_landlord(&landlord, coin_info, dividend_info, &coin_mint, &authority)?;
        let held = landlord.held(balance);
        let eligible = delegated && held > 0 && held >= min_stake;
        if eligible && config.attestations_of(&landlord) < REQUIRED_ATTESTATIONS {
            // Would count but hasn't had all its reads: pending, not zero. The
            // refresher can still read it, and it can be counted later in the
            // round (or counts zero if the round times out first).
            continue;
        }
        let counted = if eligible { held } else { 0 };

        // The next count credits at most this, and only after fresh reads.
        landlord.snapshot = if delegated { balance } else { 0 };
        landlord.snapshot_valid = delegated;
        landlord.attestations = 0;
        landlord.counted_round = round;
        landlord.counted_amount = counted;
        landlord.exit(program_id)?;

        config.count.counted = config.count.counted.checked_add(1).ok_or(EndowmentError::Overflow)?;
        // Saturating: the result is capped at 100% anyway, and an overflow must
        // never block the rest of the round (R2-CNT-11).
        config.count.committed = config.count.committed.saturating_add(counted);
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

/// Remaining accounts, three per landlord, any number, any order: the landlord
/// record (writable), its registered coin account and its registered dividend
/// account (either may be closed). Closed landlord records are skipped.
#[derive(Accounts)]
pub struct RefreshLandlords<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// Anyone. When it is the endowment's refresher, the landlords read are
    /// attested (see the module docs).
    pub caller: Signer<'info>,
}

/// Decrease-only: lowers each landlord's recorded balance to what it holds now,
/// and drops the record of any landlord no longer delegated, so the next count
/// credits no more than that. Anyone may call it, as often as they like: it can
/// never raise anything. Only the refresher's calls also attest (a read at
/// least MIN_ATTEST_SPACING_SECS after that landlord's previous one adds one to
/// its reads), and those are listed in `LandlordsAttested`.
pub fn handle_refresh_landlords<'info>(ctx: Context<'info, RefreshLandlords<'info>>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let program_id = ctx.program_id;
    let accounts = ctx.remaining_accounts;
    let config = &mut ctx.accounts.config;
    let coin_mint = config.coin_mint;
    let refresher = config.params.refresher;
    let attest = refresher != Pubkey::default() && ctx.accounts.caller.key() == refresher;
    require!(!accounts.is_empty() && accounts.len() % 3 == 0, EndowmentError::InvalidCountAccount);

    let authority = Pubkey::create_program_address(
        &[AUTHORITY_SEED, config_key.as_ref(), &[config.authority_bump]],
        program_id,
    )
    .map_err(|_| error!(EndowmentError::InvalidCountAccount))?;

    let mut attested: Vec<Pubkey> = Vec::new();
    for triple in accounts.chunks(3) {
        let (landlord_info, coin_info, dividend_info) = (&triple[0], &triple[1], &triple[2]);
        let Some(mut landlord) = load_landlord(landlord_info, coin_info, dividend_info, &config_key, program_id)?
        else {
            continue;
        };
        let (balance, delegated) = read_landlord(&landlord, coin_info, dividend_info, &coin_mint, &authority)?;
        if delegated {
            landlord.snapshot = landlord.snapshot.min(balance);
            if attest && landlord.attestation_epoch != config.refresher_epoch {
                // Reads under a previous refresher don't carry over (FC-R3-03).
                landlord.attestations = 0;
                landlord.attestation_epoch = config.refresher_epoch;
            }
            let spaced = landlord.attestations == 0
                || now.saturating_sub(landlord.last_attested_at) >= MIN_ATTEST_SPACING_SECS;
            if attest && spaced {
                landlord.attestations = landlord.attestations.saturating_add(1);
                landlord.last_attested_at = now;
                attested.push(landlord_info.key());
            }
        } else {
            landlord.snapshot = 0;
            landlord.snapshot_valid = false;
            landlord.attestations = 0;
        }
        landlord.exit(program_id)?;
    }
    if attest {
        config.last_attested_at = now;
        emit!(LandlordsAttested { config: config_key, landlords: attested, at: now });
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
    let timed_out = now.saturating_sub(config.count_clock_start()) >= COUNT_TIMEOUT_SECS;
    require!(complete || timed_out, EndowmentError::CountIncomplete);

    let count = config.count;
    let committed_bps = if count.supply == 0 {
        0
    } else {
        ((count.committed as u128 * 10_000) / count.supply as u128).min(10_000) as u16
    };
    config.count.open = false;
    config.apply_committed_bps(committed_bps);
    config.invalidate_reports()?;
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
