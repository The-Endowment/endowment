use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    constants::*,
    error::EndowmentError,
    events::LandlordRegistered,
    state::{Config, Landlord},
};

/// Sent in the same transaction as the landlord's
/// `Approve(dividend_account → authority)` for this endowment.
#[derive(Accounts)]
pub struct RegisterLandlord<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: this endowment's authority PDA; address check only.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        init,
        payer = owner,
        space = 8 + Landlord::INIT_SPACE,
        seeds = [LANDLORD_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump
    )]
    pub landlord: Box<Account<'info, Landlord>>,

    #[account(address = config.dividend_mint)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        associated_token::mint = dividend_mint,
        associated_token::authority = owner,
        associated_token::token_program = dividend_token_program,
        constraint = dividend_account.delegate == Some(authority.key()).into()
            @ EndowmentError::NotDelegated,
    )]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    /// Counted toward the activation threshold in each daily commitment count.
    #[account(
        associated_token::mint = coin_mint,
        associated_token::authority = owner,
        associated_token::token_program = coin_token_program,
    )]
    pub coin_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub coin_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn handle_register_landlord(ctx: Context<RegisterLandlord>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let baseline = ctx.accounts.dividend_account.amount;
    let count = ctx.accounts.config.count;
    let config_key = ctx.accounts.config.key();

    ctx.accounts.landlord.set_inner(Landlord {
        config: config_key,
        owner: ctx.accounts.owner.key(),
        dividend_account: ctx.accounts.dividend_account.key(),
        baseline,
        total_contributed: 0,
        registered_at: now,
        last_sweep_at: 0,
        bump: ctx.bumps.landlord,
        coin_account: ctx.accounts.coin_account.key(),
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
        config: config_key,
        owner: ctx.accounts.owner.key(),
        baseline,
    });
    Ok(())
}
