use anchor_lang::prelude::*;
use anchor_spl::token_interface::TokenAccount;

use crate::{
    constants::*,
    error::EndowmentError,
    state::{Config, Landlord},
};

/// Owner can renew consent even while paused. It invalidates pending reports.
#[derive(Accounts)]
pub struct ResyncBaseline<'info> {
    pub owner: Signer<'info>,
    #[account(
        mut,
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

pub fn handle_renew_reward_consent(ctx: Context<ResyncBaseline>) -> Result<()> {
    require!(ctx.accounts.config.version == CONFIG_VERSION && ctx.accounts.landlord.version == LANDLORD_VERSION,
        EndowmentError::UnsupportedCollectionVersion);
    let consent_id = ctx.accounts.config.new_consent()?;
    let landlord = &mut ctx.accounts.landlord;
    landlord.consent_id = consent_id;
    landlord.registered_at = Clock::get()?.unix_timestamp;
    emit!(crate::events::CollectionConsent {
        config: ctx.accounts.config.key(), owner: landlord.owner, consent_id,
    });
    Ok(())
}
