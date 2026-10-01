use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::get_associated_token_address_with_program_id,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    constants::*,
    error::EndowmentError,
    raydium::CPMM_PROGRAM_ID,
    state::{Config, Landlord},
};

/// Shared authenticated collection accounts. The legacy sweep entrypoint is disabled.
#[derive(Accounts)]
pub struct Sweep<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: this endowment's authority PDA, which signs as the delegate.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        mut,
        seeds = [LANDLORD_SEED, config.key().as_ref(), landlord.owner.as_ref()],
        bump = landlord.bump,
        has_one = config,
    )]
    pub landlord: Box<Account<'info, Landlord>>,

    #[account(address = config.dividend_mint)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        address = landlord.dividend_account,
        constraint = dividend_account.owner == landlord.owner @ EndowmentError::NotDelegated,
    )]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = dividend_mint,
        associated_token::authority = authority,
        associated_token::token_program = dividend_token_program,
    )]
    pub dividend_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// What a buyback would trade through; read only, to check it can.
    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    /// The direct coin vault: authenticate its spendable balance and frozen state.
    #[account(
        address = get_associated_token_address_with_program_id(
            &authority.key(),
            &config.coin_mint,
            coin_mint.to_account_info().owner,
        ) @ EndowmentError::WrongPool,
        token::mint = coin_mint,
        token::authority = authority,
        owner = *coin_mint.to_account_info().owner,
    )]
    pub coin_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the endowment's pool; parsed by `ensure_tradeable`.
    #[account(address = config.pool @ EndowmentError::WrongPool, owner = CPMM_PROGRAM_ID)]
    pub pool_state: UncheckedAccount<'info>,
    /// CHECK: the pool's AMM config; checked against the pool.
    #[account(owner = CPMM_PROGRAM_ID)]
    pub amm_config: UncheckedAccount<'info>,
    /// CHECK: the pool's dividend vault; checked against the pool.
    pub pool_dividend_vault: UncheckedAccount<'info>,
    /// CHECK: the pool's coin vault; checked against the pool.
    pub pool_coin_vault: UncheckedAccount<'info>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
}
