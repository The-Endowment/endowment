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

/// A circuit breaker, not a switch: a pause lasts MAX_PAUSE_SECONDS and can't be
/// extended, and a new one can only start PAUSE_COOLDOWN_SECONDS after the last
/// one ended. So the guardian can stop an endowment at most half the time, and
/// never for good. Leaving (revoke, deregister) is never paused.
pub fn handle_pause(ctx: Context<Pause>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(
        config.paused_until == 0 || now >= config.paused_until.saturating_add(PAUSE_COOLDOWN_SECONDS),
        EndowmentError::PauseCooldown
    );
    config.paused_until = now + MAX_PAUSE_SECONDS;
    emit!(PauseChanged { config: config_key, paused_until: config.paused_until });
    Ok(())
}

/// Lifting a pause early takes the admin. The cooldown runs from now.
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
