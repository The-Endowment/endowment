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
    pub config: Account<'info, Config>,
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
    pub config: Account<'info, Config>,
}

/// Blocks cranks for MAX_PAUSE_SECONDS. Calling it again restarts the clock,
/// and every call is a public event.
pub fn handle_pause(ctx: Context<Pause>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    ctx.accounts.config.paused_until = now + MAX_PAUSE_SECONDS;
    emit!(PauseChanged { config: config_key, paused_until: ctx.accounts.config.paused_until });
    Ok(())
}

/// Lifting a pause early takes the admin, so a single guardian key can stop
/// the endowment but can't restart it on its own.
pub fn handle_unpause(ctx: Context<Unpause>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    ctx.accounts.config.paused_until = 0;
    emit!(PauseChanged { config: config_key, paused_until: 0 });
    Ok(())
}
