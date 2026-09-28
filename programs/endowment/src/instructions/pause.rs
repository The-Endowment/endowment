use anchor_lang::prelude::*;

use crate::{constants::*, error::EndowmentError, events::PauseChanged, state::Config};

#[derive(Accounts)]
pub struct SetPause<'info> {
    pub guardian: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = guardian @ EndowmentError::NotGuardian,
    )]
    pub config: Account<'info, Config>,
}

/// Blocks cranks for MAX_PAUSE_SECONDS. Calling it again restarts the clock,
/// and every call is a public event.
pub fn handle_pause(ctx: Context<SetPause>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    ctx.accounts.config.paused_until = now + MAX_PAUSE_SECONDS;
    emit!(PauseChanged {
        paused_until: ctx.accounts.config.paused_until,
    });
    Ok(())
}

pub fn handle_unpause(ctx: Context<SetPause>) -> Result<()> {
    ctx.accounts.config.paused_until = 0;
    emit!(PauseChanged { paused_until: 0 });
    Ok(())
}
