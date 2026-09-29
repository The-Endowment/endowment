use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::*,
    error::EndowmentError,
    events::Swept,
    state::{Config, Landlord},
    transfer::hook_enabled,
};

/// Permissionless: anyone may crank a sweep. Funds can only move from the
/// landlord's delegated dividend account into this endowment's dividend vault,
/// and only the part above the landlord's baseline. A sweep never changes the
/// baseline: a dip below it sweeps nothing and leaves it where it is.
///
/// Sweeps keep running after the milestone; they stop for good only if the admin
/// retires the endowment. They fail closed if the dividend mint's transfer hook
/// is ever switched on, since buybacks can't spend such a dividend through Raydium.
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
    #[account(mut, address = landlord.dividend_account)]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = dividend_mint,
        associated_token::authority = authority,
        associated_token::token_program = dividend_token_program,
    )]
    pub dividend_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
}

pub fn handle_sweep(ctx: Context<Sweep>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(!config.retired, EndowmentError::Retired);
    require!(config.active, EndowmentError::NotActive);
    require!(
        !hook_enabled(&ctx.accounts.dividend_mint.to_account_info())?,
        EndowmentError::TransferHookEnabled
    );

    let dividend_account = &ctx.accounts.dividend_account;
    require!(
        dividend_account.delegate == Some(ctx.accounts.authority.key()).into(),
        EndowmentError::NotDelegated
    );
    let amount = ctx.accounts.landlord.sweepable(dividend_account.amount, dividend_account.delegated_amount);
    if amount == 0 {
        return Ok(());
    }

    let vault_before = ctx.accounts.dividend_vault.amount;
    let bump = [config.authority_bump];
    let seeds = Config::authority_seeds(&config_key, &bump);
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.dividend_token_program.key(),
            TransferChecked {
                from: ctx.accounts.dividend_account.to_account_info(),
                mint: ctx.accounts.dividend_mint.to_account_info(),
                to: ctx.accounts.dividend_vault.to_account_info(),
                authority: ctx.accounts.authority.to_account_info(),
            },
            &[&seeds],
        ),
        amount,
        ctx.accounts.dividend_mint.decimals,
    )?;
    // What actually arrived, net of any transfer fee on the dividend.
    ctx.accounts.dividend_vault.reload()?;
    let received = ctx.accounts.dividend_vault.amount.saturating_sub(vault_before);

    let landlord = &mut ctx.accounts.landlord;
    landlord.total_contributed = landlord.total_contributed.checked_add(amount).ok_or(EndowmentError::Overflow)?;
    landlord.last_sweep_at = now;
    let config = &mut ctx.accounts.config;
    config.total_swept = config.total_swept.checked_add(received).ok_or(EndowmentError::Overflow)?;

    emit!(Swept {
        config: config_key,
        owner: landlord.owner,
        amount,
        received,
        baseline: landlord.baseline,
        total_contributed: landlord.total_contributed,
    });
    Ok(())
}
