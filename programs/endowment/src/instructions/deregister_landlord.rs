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
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,
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
    let landlord = &ctx.accounts.landlord;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    config.landlord_count = config.landlord_count.saturating_sub(1);

    // Keep an open count finishable and honest: a landlord still expected
    // leaves the expected set; one already counted takes its coin back out.
    let count = &mut config.count;
    if count.open {
        if landlord.counted_in(count.round) {
            count.committed = count.committed.saturating_sub(landlord.counted_balance);
            count.counted = count.counted.saturating_sub(1);
            count.expected = count.expected.saturating_sub(1);
        } else if landlord.counted_round < count.round {
            count.expected = count.expected.saturating_sub(1);
        }
    }

    emit!(LandlordDeregistered {
        config: config_key,
        owner: ctx.accounts.owner.key(),
        total_contributed: landlord.total_contributed,
    });
    Ok(())
}
