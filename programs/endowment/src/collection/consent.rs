use super::state::*;
use crate::{
    constants::*,
    error::EndowmentError,
    state::{Config, Landlord},
};
use anchor_lang::prelude::*;
use anchor_spl::token_interface::TokenAccount;

#[derive(Accounts)]
pub struct EnableCollection<'info> {
    pub owner: Signer<'info>,
    #[account(seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [POLICY_SEED, config.key().as_ref()], bump = policy.bump, has_one = config)]
    pub policy: Box<Account<'info, CollectionPolicy>>,
    #[account(mut, seeds = [CONSENT_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump = consent.bump, has_one = config, has_one = owner)]
    pub consent: Box<Account<'info, CollectionConsent>>,
    #[account(mut, seeds = [LANDLORD_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump = landlord.bump, has_one = config, has_one = owner)]
    pub landlord: Box<Account<'info, Landlord>>,
    #[account(address = landlord.dividend_account,
        constraint = dividend_account.owner == owner.key() @ EndowmentError::NotDelegated)]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,
}

pub fn enable(ctx: Context<EnableCollection>) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(!config.retired && !config.milestone_reached, EndowmentError::Completed);
    let consent = &mut ctx.accounts.consent;
    consent.disable()?;
    consent.enabled = true;
    consent.started_at = Clock::get()?.unix_timestamp;
    let landlord = &mut ctx.accounts.landlord;
    // Every new consent protects everything currently in the wallet, including
    // refunds. Old receipts now have a different epoch and can only be refunded.
    landlord.baseline = ctx.accounts.dividend_account.amount;
    landlord.index_at = config.reward_index;
    landlord.allowance = 0;
    Ok(())
}

#[derive(Accounts)]
pub struct DisableCollection<'info> {
    pub owner: Signer<'info>,
    #[account(mut, seeds = [CONSENT_SEED, consent.config.as_ref(), owner.key().as_ref()],
        bump = consent.bump, has_one = owner)]
    pub consent: Box<Account<'info, CollectionConsent>>,
}
pub fn disable(ctx: Context<DisableCollection>) -> Result<()> {
    ctx.accounts.consent.disable()
}
