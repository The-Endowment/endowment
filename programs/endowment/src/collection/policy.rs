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
    require!(CollectionPolicy::valid_roles(&collector, &reviewer), EndowmentError::InvalidCollectionPolicy);
    ctx.accounts.policy.set_inner(CollectionPolicy {
        config: ctx.accounts.config.key(),
        collector,
        reviewer,
        bump: ctx.bumps.policy,
        pending: 0,
        released: 0,
        refunded: 0,
        pending_collector: Pubkey::default(),
        pending_reviewer: Pubkey::default(),
        pending_roles_at: 0,
    });
    Ok(())
}

/// The admin proposes or applies a change of collector and reviewer.
#[derive(Accounts)]
pub struct ChangeCollectionRoles<'info> {
    #[account(address = config.admin @ EndowmentError::NotAdmin)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [POLICY_SEED, config.key().as_ref()], bump = policy.bump, has_one = config)]
    pub policy: Box<Account<'info, CollectionPolicy>>,
}

/// Starts the timelock on a new collector and reviewer (a lost or leaked key
/// can be replaced), replacing any earlier proposal. Two default keys cancel.
/// Receipts already pending are unaffected: the new reviewer reviews them.
pub fn propose_roles(ctx: Context<ChangeCollectionRoles>, collector: Pubkey, reviewer: Pubkey) -> Result<()> {
    let config = ctx.accounts.config.key();
    let policy = &mut ctx.accounts.policy;
    let cancel = collector == Pubkey::default() && reviewer == Pubkey::default();
    require!(
        cancel
            || (CollectionPolicy::valid_roles(&collector, &reviewer)
                && reviewer != policy.collector
                && collector != policy.reviewer),
        EndowmentError::InvalidCollectionPolicy
    );
    policy.pending_collector = collector;
    policy.pending_reviewer = reviewer;
    policy.pending_roles_at = if cancel {
        0
    } else {
        Clock::get()?.unix_timestamp.checked_add(PARAM_TIMELOCK_SECONDS).ok_or(EndowmentError::Overflow)?
    };
    emit!(CollectionRolesProposed { config, collector, reviewer, effective_at: policy.pending_roles_at });
    Ok(())
}

pub fn apply_roles(ctx: Context<ChangeCollectionRoles>) -> Result<()> {
    let config = ctx.accounts.config.key();
    let policy = &mut ctx.accounts.policy;
    require!(policy.pending_roles_at != 0, EndowmentError::NoPendingParams);
    require!(Clock::get()?.unix_timestamp >= policy.pending_roles_at, EndowmentError::TimelockNotElapsed);
    policy.collector = policy.pending_collector;
    policy.reviewer = policy.pending_reviewer;
    policy.pending_collector = Pubkey::default();
    policy.pending_reviewer = Pubkey::default();
    policy.pending_roles_at = 0;
    emit!(CollectionRolesChanged { config, collector: policy.collector, reviewer: policy.reviewer });
    Ok(())
}
