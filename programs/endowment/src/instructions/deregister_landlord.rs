use anchor_lang::prelude::*;

use crate::{
    constants::*,
    events::LandlordDeregistered,
    state::{Config, Landlord},
};

/// Closes the landlord record and refunds the rent. Always allowed, even while
/// paused or retired. To stop sweeps immediately, also revoke the token
/// delegation (the website does both in one transaction). Leaving during an open
/// count takes the landlord out of that count's tally.
#[derive(Accounts)]
pub struct DeregisterLandlord<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    #[account(
        mut,
        close = owner,
        seeds = [LANDLORD_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump = landlord.bump,
        has_one = owner,
        has_one = config,
    )]
    pub landlord: Box<Account<'info, Landlord>>,
}

pub fn handle_deregister_landlord(ctx: Context<DeregisterLandlord>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    let landlord = &ctx.accounts.landlord;
    config.remove_from_round(landlord);
    config.landlord_count = config.landlord_count.saturating_sub(1);
    emit!(LandlordDeregistered {
        config: config.key(),
        owner: ctx.accounts.owner.key(),
        total_contributed: landlord.total_contributed,
    });
    Ok(())
}
