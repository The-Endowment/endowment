use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::EndowmentError,
    events::{
        AdminAccepted, AdminProposed, AdminRenounced, GuardianChanged, ParamsApplied, ParamsCancelled,
        ParamsProposed, RefresherResigned, RetireProposed, Retired,
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
pub struct ResignRefresher<'info> {
    pub refresher: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
        constraint = config.params.refresher != Pubkey::default()
            && config.params.refresher == refresher.key() @ EndowmentError::NotRefresher,
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
    params.validate()?;
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
    let refresher_changes = pending.params.refresher != config.params.refresher;
    let swept_before = config.sweeps_on(now);
    config.params = pending.params;
    config.pending = PendingParams::default();
    if refresher_changes {
        // Reads made under the previous refresher stop counting, and neither an
        // open round nor the last count's result can switch sweeps back on
        // (FC-R3-03, PR #1).
        config.retire_refresher_reads();
    }
    // Keep the buy allowance within the (possibly smaller) new per-transaction cap.
    config.buy_allowance = config.buy_allowance.min(config.params.max_buy_per_tx);
    // New thresholds apply to the last count at once, unless the refresher is
    // gone or changed: that count rests on reads that no longer count, so
    // sweeps stay off until a count under the new refresher (FC-R3-03).
    if config.params.refresher == Pubkey::default() || refresher_changes {
        config.active = false;
    } else {
        let last = config.last_count_bps;
        config.apply_committed_bps(last);
    }
    // New parameters switched sweeps on: rewards paid while they were off stay
    // with landlords.
    if !swept_before && config.sweeps_on(now) {
        config.reward_credit_ok = false;
    }
    emit!(ParamsApplied { config: config_key, params: config.params, active: config.active });
    Ok(())
}

/// One-way: stops landlord sweeps and new registrations for good. It never moves
/// funds and doesn't change how buybacks spend what the vault holds. Timelocked
/// like a parameter change: the first call proposes it (announced by an event),
/// a call after PARAM_TIMELOCK_SECONDS carries it out, and `cancel_params`
/// withdraws it in between. Like a parameter proposal it expires
/// PARAM_EXPIRY_SECONDS after maturing; a call after that proposes it afresh.
pub fn handle_retire(ctx: Context<AdminOnly>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    if config.retired {
        return Ok(());
    }
    let expired = config.retire_at != 0 && now > config.retire_at.saturating_add(PARAM_EXPIRY_SECONDS);
    if config.retire_at == 0 || expired {
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
/// activation thresholds, so sweeps can't be frozen on, the reward allowance
/// on, so they can't be frozen uncapped, no pending change
/// (cancel it first), so nothing half-decided is left behind, and (unless
/// retired) a refresher, without which nobody could ever count again.
///
/// Three live roles remain: the collector and reviewer (see `collection`),
/// which can no longer be replaced, and the refresher, whose reads decide who
/// counts (see `count`). After renounce the refresher can't be replaced, only resign
/// (`resign_refresher`), which switches sweeps off at once and voids its
/// reads: a leaked or distrusted refresher key can always be retired by
/// whoever holds it, and never handed to anyone else.
pub fn handle_renounce_admin(ctx: Context<AdminOnly>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(config.pending.effective_at == 0 && config.retire_at == 0, EndowmentError::PendingChange);
    require!(
        config.retired || config.params.refresher != Pubkey::default(),
        EndowmentError::NoRefresher
    );
    require!(
        config.retired
            || (config.params.activate_bps >= MIN_RENOUNCE_ACTIVATE_BPS
                && config.params.deactivate_bps >= MIN_RENOUNCE_DEACTIVATE_BPS),
        EndowmentError::RenounceThresholds
    );
    // And the reward allowance on, so sweeps can never be frozen uncapped.
    require!(
        config.retired
            || (config.params.allowance_margin_bps == ALLOWANCE_MARGIN_BPS
                && config.params.max_rewards_per_day
                    <= config.params.max_buy_per_day.saturating_mul(MAX_RENOUNCE_REWARDS_TO_BUYS)),
        EndowmentError::InvalidAllowance
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

/// The refresher gives up its role, at once and for good (until the admin, if
/// any, sets another through the timelock). Resigning is the only change the
/// refresher can make, and it can only ever move toward fewer landlords
/// counting, so it needs no timelock: it's how a refresher whose key may have
/// leaked shuts it off, including after the admin has renounced. It:
/// - switches sweeps off at once, rather than waiting for a count that no
///   longer has a refresher to run it (FC-R3-03);
/// - voids every read it made, so nothing it attested can still be counted,
///   in an open round or later (FC-R3-03);
/// - strips it from a pending parameter proposal, so a change proposed while it
///   was trusted can't bring it back when applied (FC-R3-02).
pub fn handle_resign_refresher(ctx: Context<ResignRefresher>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    let refresher = config.params.refresher;
    config.params.refresher = Pubkey::default();
    if config.pending.effective_at != 0 && config.pending.params.refresher == refresher {
        config.pending.params.refresher = Pubkey::default();
    }
    config.retire_refresher_reads();
    emit!(RefresherResigned { config: config_key, refresher });
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
