use super::state::*;
use crate::{constants::*, error::EndowmentError, state::Config};
use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

#[derive(Accounts)]
pub struct InitializeCollection<'info> {
    #[account(mut, address = config.admin @ EndowmentError::NotAdmin)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(init, payer = admin, space = 8 + CollectionPolicy::INIT_SPACE,
        seeds = [POLICY_SEED, config.key().as_ref()], bump)]
    pub policy: Box<Account<'info, CollectionPolicy>>,
    #[account(address = config.dividend_mint)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(init_if_needed, payer = admin, associated_token::mint = dividend_mint,
        associated_token::authority = policy, associated_token::token_program = dividend_token_program)]
    pub pending_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn initialize(ctx: Context<InitializeCollection>, collector: Pubkey, reviewer: Pubkey) -> Result<()> {
    require!(
        collector != Pubkey::default() && reviewer != Pubkey::default() && collector != reviewer,
        EndowmentError::InvalidCollectionPolicy
    );
    ctx.accounts.policy.set_inner(CollectionPolicy {
        config: ctx.accounts.config.key(),
        collector,
        reviewer,
        bump: ctx.bumps.policy,
        pending: 0,
        released: 0,
        refunded: 0,
    });
    Ok(())
}
