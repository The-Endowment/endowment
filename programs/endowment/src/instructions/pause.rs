use anchor_lang::prelude::*;

use crate::{constants::*, error::EndowmentError, events::PauseChanged, state::Config};

#[derive(Accounts)]
pub struct Pause<'info> {
    pub guardian: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
        has_one = guardian @ EndowmentError::NotGuardian,
    )]
    pub config: Box<Account<'info, Config>>,
}

#[derive(Accounts)]
pub struct Unpause<'info> {
    pub admin: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
        has_one = admin @ EndowmentError::NotAdmin,
    )]
    pub config: Box<Account<'info, Config>>,
}

/// Stops new collection and spending until explicit admin resume. There is no
/// timeout or cooldown: another incident must always be stoppable immediately.
/// Holder exits and all permitted refunds remain available.
pub fn handle_pause(ctx: Context<Pause>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::PauseCooldown);
    // Never revive an earlier incident's receipts if the chain clock moves
    // backwards. Same-second collection/resume is conservatively refundable.
    config.pause_started_at = config.pause_started_at.max(now);
    config.paused_until = INCIDENT_PAUSE_UNTIL;
    config.reward_credit_ok = false;
    emit!(PauseChanged { config: config_key, paused_until: config.paused_until });
    Ok(())
}

/// Only the admin can resume after investigating an incident. The incident
/// cutoff is retained: pre-pause receipts never become spendable again.
pub fn handle_unpause(ctx: Context<Unpause>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    if config.is_paused(now) {
        config.paused_until = now;
    }
    emit!(PauseChanged { config: config_key, paused_until: config.paused_until });
    Ok(())
}
