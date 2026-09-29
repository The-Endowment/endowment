use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    constants::*,
    error::EndowmentError,
    events::LandlordRegistered,
    state::{Config, Landlord},
};

/// Sent in the same transaction as the landlord's `Approve(pump_account → authority)`.
#[derive(Accounts)]
pub struct RegisterLandlord<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: PDA address check only.
    #[account(seeds = [AUTHORITY_SEED], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        init,
        payer = owner,
        space = 8 + Landlord::INIT_SPACE,
        seeds = [LANDLORD_SEED, owner.key().as_ref()],
        bump
    )]
    pub landlord: Box<Account<'info, Landlord>>,

    #[account(address = config.pump_mint)]
    pub pump_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        associated_token::mint = pump_mint,
        associated_token::authority = owner,
        associated_token::token_program = pump_token_program,
        constraint = pump_account.delegate == Some(authority.key()).into()
            @ EndowmentError::NotDelegated,
    )]
    pub pump_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = config.penis_mint)]
    pub penis_mint: Box<InterfaceAccount<'info, Mint>>,
    /// Counted toward the activation threshold in each daily commitment count.
    #[account(
        associated_token::mint = penis_mint,
        associated_token::authority = owner,
        associated_token::token_program = penis_token_program,
    )]
    pub penis_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub pump_token_program: Interface<'info, TokenInterface>,
    pub penis_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn handle_register_landlord(ctx: Context<RegisterLandlord>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let baseline = ctx.accounts.pump_account.amount;
    let count = ctx.accounts.config.count;

    ctx.accounts.landlord.set_inner(Landlord {
        owner: ctx.accounts.owner.key(),
        pump_account: ctx.accounts.pump_account.key(),
        baseline,
        total_contributed: 0,
        registered_at: now,
        last_sweep_at: 0,
        bump: ctx.bumps.landlord,
        penis_account: ctx.accounts.penis_account.key(),
        // A landlord who joins while a count is open sits that round out: it
        // isn't expected, so it can't hold the round up or be counted twice.
        counted_round: count.round,
        counted_balance: 0,
        joined_round: if count.open { count.round } else { 0 },
    });

    let config = &mut ctx.accounts.config;
    config.landlord_count = config
        .landlord_count
        .checked_add(1)
        .ok_or(EndowmentError::Overflow)?;

    emit!(LandlordRegistered {
        owner: ctx.accounts.owner.key(),
        baseline,
    });
    Ok(())
}
