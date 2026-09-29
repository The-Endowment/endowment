use anchor_lang::prelude::*;

use crate::{
    constants::*,
    events::LandlordDeregistered,
    state::{Config, Landlord, Roster},
};

/// Closes the landlord record, leaves the roster, and refunds the rent. Always
/// allowed, even while paused or retired. To stop sweeps immediately, also
/// revoke the token delegation (the website does both in one transaction).
#[derive(Accounts)]
pub struct DeregisterLandlord<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [ROSTER_SEED, config.key().as_ref()], bump = config.roster_bump)]
    pub roster: Account<'info, Roster>,
    #[account(
        mut,
        close = owner,
        seeds = [LANDLORD_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump = landlord.bump,
        has_one = owner,
        has_one = config,
    )]
    pub landlord: Account<'info, Landlord>,
}

pub fn handle_deregister_landlord(ctx: Context<DeregisterLandlord>) -> Result<()> {
    let owner = ctx.accounts.owner.key();
    let roster = &mut ctx.accounts.roster;
    if let Some(i) = roster.position(&owner) {
        roster.entries.remove(i);
    }
    emit!(LandlordDeregistered {
        config: ctx.accounts.config.key(),
        owner,
        total_contributed: ctx.accounts.landlord.total_contributed,
    });
    Ok(())
}
