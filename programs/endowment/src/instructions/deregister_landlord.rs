use anchor_lang::prelude::*;

use crate::{
    constants::*,
    events::LandlordDeregistered,
    state::{Config, Landlord},
};

/// Closes the landlord record and refunds its rent. Stopping sweeps does not
/// need this: revoking the token delegation alone is enough.
#[derive(Accounts)]
pub struct DeregisterLandlord<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        close = owner,
        seeds = [LANDLORD_SEED, owner.key().as_ref()],
        bump = landlord.bump,
        has_one = owner,
    )]
    pub landlord: Account<'info, Landlord>,
}

pub fn handle_deregister_landlord(ctx: Context<DeregisterLandlord>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.landlord_count = config.landlord_count.saturating_sub(1);

    emit!(LandlordDeregistered {
        owner: ctx.accounts.owner.key(),
        total_contributed: ctx.accounts.landlord.total_contributed,
    });
    Ok(())
}
