use crate::{constants::*, error::EndowmentError, reports::ReporterPolicy, state::Config};
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct InitializeReporter<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()], bump = config.bump,
        has_one = admin @ EndowmentError::NotAdmin)]
    pub config: Box<Account<'info, Config>>,
    #[account(init, payer = admin, space = 8 + ReporterPolicy::INIT_SPACE,
        seeds = [REPORTER_SEED, config.key().as_ref()], bump)]
    pub reporter_policy: Box<Account<'info, ReporterPolicy>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ManageReporter<'info> {
    pub caller: Signer<'info>,
    #[account(seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [REPORTER_SEED, config.key().as_ref()], bump = reporter_policy.bump, has_one = config)]
    pub reporter_policy: Box<Account<'info, ReporterPolicy>>,
}

pub fn initialize(ctx: Context<InitializeReporter>, reporter: Pubkey) -> Result<()> {
    require!(
        ctx.accounts.config.version == CONFIG_VERSION,
        EndowmentError::UnsupportedCollectionVersion
    );
    require_keys_neq!(reporter, Pubkey::default(), EndowmentError::InvalidReporter);
    ctx.accounts.reporter_policy.set_inner(ReporterPolicy {
        config: ctx.accounts.config.key(),
        reporter,
        pending: Pubkey::default(),
        effective_at: 0,
        epoch: 1,
        disabled: false,
        bump: ctx.bumps.reporter_policy,
    });
    ctx.accounts.reporter_policy.emit_change();
    Ok(())
}

pub fn propose(ctx: Context<ManageReporter>, reporter: Pubkey) -> Result<()> {
    require_keys_eq!(
        ctx.accounts.caller.key(),
        ctx.accounts.config.admin,
        EndowmentError::NotAdmin
    );
    require_keys_neq!(reporter, Pubkey::default(), EndowmentError::InvalidReporter);
    let policy = &mut ctx.accounts.reporter_policy;
    policy.pending = reporter;
    policy.effective_at = Clock::get()?
        .unix_timestamp
        .checked_add(PARAM_TIMELOCK_SECONDS)
        .ok_or(EndowmentError::Overflow)?;
    policy.emit_change();
    Ok(())
}

pub fn cancel(ctx: Context<ManageReporter>) -> Result<()> {
    require_keys_eq!(
        ctx.accounts.caller.key(),
        ctx.accounts.config.admin,
        EndowmentError::NotAdmin
    );
    let policy = &mut ctx.accounts.reporter_policy;
    policy.pending = Pubkey::default();
    policy.effective_at = 0;
    policy.emit_change();
    Ok(())
}

pub fn apply(ctx: Context<ManageReporter>) -> Result<()> {
    let config = &ctx.accounts.config;
    // Renunciation freezes the role even if a proposal was pending.
    require_keys_neq!(
        config.admin,
        Pubkey::default(),
        EndowmentError::ReporterFrozen
    );
    let now = Clock::get()?.unix_timestamp;
    let policy = &mut ctx.accounts.reporter_policy;
    require!(policy.effective_at != 0, EndowmentError::NoPendingParams);
    require!(
        now >= policy.effective_at,
        EndowmentError::TimelockNotElapsed
    );
    require!(
        now <= policy.effective_at.saturating_add(PARAM_EXPIRY_SECONDS),
        EndowmentError::ProposalExpired
    );
    require!(
        now >= policy
            .effective_at
            .saturating_add(PARAM_APPLY_GRACE_SECONDS)
            || ctx.accounts.caller.key() == config.admin,
        EndowmentError::ApplyGrace
    );
    policy.invalidate()?;
    policy.reporter = policy.pending;
    policy.pending = Pubkey::default();
    policy.effective_at = 0;
    policy.disabled = false;
    policy.emit_change();
    Ok(())
}

/// The admin or reporter stops collection immediately. The guardian retains
/// only its bounded `pause` power. Restart or replacement always requires the
/// admin and a fresh 72-hour proposal; after renunciation this is permanent.
/// Repeating the stop is a no-op, so a disabled reporter cannot cancel the
/// admin's recovery proposal. The admin can cancel it with `cancel_reporter`.
pub fn disable(ctx: Context<ManageReporter>) -> Result<()> {
    let caller = ctx.accounts.caller.key();
    let config = &ctx.accounts.config;
    let policy = &mut ctx.accounts.reporter_policy;
    require!(
        caller == config.admin || caller == policy.reporter,
        EndowmentError::NotReporter
    );
    if policy.disabled {
        return Ok(());
    }
    policy.invalidate()?;
    policy.disabled = true;
    policy.pending = Pubkey::default();
    policy.effective_at = 0;
    policy.emit_change();
    Ok(())
}
