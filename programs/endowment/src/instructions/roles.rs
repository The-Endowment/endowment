use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::EndowmentError,
    events::{
        AdminAccepted, AdminProposed, AdminRenounced, GuardianChanged, ParamsApplied, ParamsCancelled,
        ParamsProposed, RetireProposed, Retired,
    },
    state::{Config, Params, PendingParams},
};

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
        has_one = admin @ EndowmentError::NotAdmin,
    )]
    pub config: Box<Account<'info, Config>>,
}

/// Applies a proposed parameter change once its timelock ends: only the admin
/// for the first PARAM_APPLY_GRACE_SECONDS (so the admin can still cancel, or
/// cancel and renounce, without being raced), then anyone, until the proposal
/// expires PARAM_EXPIRY_SECONDS after maturing.
#[derive(Accounts)]
pub struct ApplyParams<'info> {
    pub caller: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
}

#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    pub new_admin: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
        constraint = config.pending_admin != Pubkey::default()
            && config.pending_admin == new_admin.key() @ EndowmentError::NotPendingAdmin,
    )]
    pub config: Box<Account<'info, Config>>,
}

/// Proposes a full new parameter set, within the hard-coded bounds. It takes
/// effect no sooner than PARAM_TIMELOCK_SECONDS later, announced by an event.
/// A new proposal replaces a pending one and restarts the clock.
pub fn handle_propose_params(ctx: Context<AdminOnly>, params: Params) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    params.validate(config.donation_bps)?;
    let effective_at = now + PARAM_TIMELOCK_SECONDS;
    config.pending = PendingParams { params, effective_at };
    emit!(ParamsProposed { config: config_key, params, effective_at });
    Ok(())
}

/// Cancels a pending parameter change and any pending retirement.
pub fn handle_cancel_params(ctx: Context<AdminOnly>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(config.pending.effective_at != 0 || config.retire_at != 0, EndowmentError::NoPendingParams);
    config.pending = PendingParams::default();
    config.retire_at = 0;
    emit!(ParamsCancelled { config: config_key });
    Ok(())
}

pub fn handle_apply_params(ctx: Context<ApplyParams>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    let pending = config.pending;
    require!(pending.effective_at != 0, EndowmentError::NoPendingParams);
    require!(now >= pending.effective_at, EndowmentError::TimelockNotElapsed);
    require!(
        now <= pending.effective_at.saturating_add(PARAM_EXPIRY_SECONDS),
        EndowmentError::ProposalExpired
    );
    require!(
        now >= pending.effective_at.saturating_add(PARAM_APPLY_GRACE_SECONDS)
            || ctx.accounts.caller.key() == config.admin,
        EndowmentError::ApplyGrace
    );
    config.params = pending.params;
    config.pending = PendingParams::default();
    // Keep the buy allowance within the (possibly smaller) new per-transaction cap.
    config.buy_allowance = config.buy_allowance.min(config.params.max_buy_per_tx);
    // New thresholds apply to the last count at once.
    let last = config.last_count_bps;
    config.apply_committed_bps(last);
    emit!(ParamsApplied { config: config_key, params: config.params, active: config.active });
    Ok(())
}

/// One-way: stops landlord sweeps and new registrations for good. It never moves
/// funds and doesn't change how buybacks spend what the vault holds. Timelocked
/// like a parameter change: the first call proposes it (announced by an event),
/// a call after PARAM_TIMELOCK_SECONDS carries it out, and `cancel_params`
/// withdraws it in between.
pub fn handle_retire(ctx: Context<AdminOnly>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    if config.retired {
        return Ok(());
    }
    if config.retire_at == 0 {
        config.retire_at = now + PARAM_TIMELOCK_SECONDS;
        emit!(RetireProposed { config: config_key, effective_at: config.retire_at });
        return Ok(());
    }
    require!(now >= config.retire_at, EndowmentError::RetireNotReady);
    config.retired = true;
    config.retire_at = 0;
    emit!(Retired { config: config_key });
    Ok(())
}

/// One-way: gives up the admin role for good, freezing every parameter as it
/// stands. It also clears the guardian and any pending change, so no key is left
/// that can pause or reconfigure the endowment. It requires production
/// activation thresholds, so sweeps can't be frozen on, and no pending change
/// (cancel it first), so nothing half-decided is left behind.
pub fn handle_renounce_admin(ctx: Context<AdminOnly>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(config.pending.effective_at == 0 && config.retire_at == 0, EndowmentError::PendingChange);
    require!(
        config.retired
            || (config.params.activate_bps >= MIN_RENOUNCE_ACTIVATE_BPS
                && config.params.deactivate_bps >= MIN_RENOUNCE_DEACTIVATE_BPS),
        EndowmentError::RenounceThresholds
    );
    config.admin = Pubkey::default();
    config.pending_admin = Pubkey::default();
    config.guardian = Pubkey::default();
    config.pending = PendingParams::default();
    emit!(AdminRenounced { config: config_key });
    emit!(GuardianChanged { config: config_key, guardian: Pubkey::default() });
    Ok(())
}

/// Immediate: the guardian can only pause (within its limits), so rotating it
/// is low risk. `Pubkey::default()` removes it.
pub fn handle_set_guardian(ctx: Context<AdminOnly>, new_guardian: Pubkey) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    ctx.accounts.config.guardian = new_guardian;
    emit!(GuardianChanged { config: config_key, guardian: new_guardian });
    Ok(())
}

/// Step one of an admin handover. Proposing `Pubkey::default()` cancels.
pub fn handle_propose_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    ctx.accounts.config.pending_admin = new_admin;
    emit!(AdminProposed { config: config_key, pending_admin: new_admin });
    Ok(())
}

/// Step two: the proposed admin must sign, so a typo can't brick the role.
pub fn handle_accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    config.admin = config.pending_admin;
    config.pending_admin = Pubkey::default();
    emit!(AdminAccepted { config: config_key, admin: config.admin });
    Ok(())
}
