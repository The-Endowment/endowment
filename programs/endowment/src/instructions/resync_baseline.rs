use anchor_lang::prelude::*;
use anchor_spl::token_interface::TokenAccount;

use crate::{
    constants::*,
    events::BaselineChanged,
    state::{Config, Landlord},
};

/// Owner-signed: sets the landlord's baseline to its dividend account's current
/// balance, so everything it holds now stays its own. The website sends this
/// with the approval whenever a landlord opts back in. Nothing else can lower a
/// baseline.
#[derive(Accounts)]
pub struct ResyncBaseline<'info> {
    pub owner: Signer<'info>,
    #[account(
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    #[account(
        mut,
        seeds = [LANDLORD_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump = landlord.bump,
        has_one = owner,
        has_one = config,
    )]
    pub landlord: Box<Account<'info, Landlord>>,
    #[account(
        address = landlord.dividend_account,
        constraint = dividend_account.owner == owner.key() @ crate::error::EndowmentError::NotDelegated,
    )]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,
}

pub fn handle_resync_baseline(ctx: Context<ResyncBaseline>) -> Result<()> {
    let new_baseline = ctx.accounts.dividend_account.amount;
    let landlord = &mut ctx.accounts.landlord;
    let old_baseline = landlord.baseline;
    landlord.baseline = new_baseline;
    emit!(BaselineChanged {
        config: ctx.accounts.config.key(),
        owner: landlord.owner,
        old_baseline,
        new_baseline,
    });
    Ok(())
}
