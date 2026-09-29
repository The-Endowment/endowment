use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    constants::*,
    error::EndowmentError,
    events::{ContributionsClosed, Swept},
    state::{Config, Landlord},
    transfer::transfer_checked_with_hook,
};

/// Permissionless: anyone may crank a sweep. Funds can only move from the
/// landlord's delegated dividend account into this endowment's dividend vault.
///
/// Remaining accounts: dividend transfer-hook extras, only if the dividend
/// mint's hook is ever switched on (see `transfer.rs`).
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
    /// Read to close contributions as soon as the cap is reached.
    #[account(
        associated_token::mint = config.coin_mint,
        associated_token::authority = authority,
        associated_token::token_program = coin_token_program,
    )]
    pub coin_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub coin_token_program: Interface<'info, TokenInterface>,
}

pub fn handle_sweep<'info>(ctx: Context<'info, Sweep<'info>>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);

    // Contributions close for good once the vault reaches the cap.
    if !config.closed && ctx.accounts.coin_vault.amount >= config.contribution_cap {
        config.closed = true;
        emit!(ContributionsClosed { config: config_key, by_cap: true, coin_held: ctx.accounts.coin_vault.amount });
        // Persist the close even though this sweep does nothing else.
        return Ok(());
    }
    require!(!config.closed, EndowmentError::ContributionsClosed);
    require!(config.active, EndowmentError::NotActive);

    let dividend_account = &ctx.accounts.dividend_account;
    require!(
        dividend_account.delegate == Some(ctx.accounts.authority.key()).into(),
        EndowmentError::NotDelegated
    );

    let (amount, baseline) = ctx
        .accounts
        .landlord
        .sweepable(dividend_account.amount, dividend_account.delegated_amount);
    ctx.accounts.landlord.baseline = baseline;
    if amount == 0 {
        return Ok(());
    }

    let bump = [ctx.accounts.config.authority_bump];
    let seeds = Config::authority_seeds(&config_key, &bump);
    transfer_checked_with_hook(
        &ctx.accounts.dividend_token_program.to_account_info(),
        &ctx.accounts.dividend_account.to_account_info(),
        &ctx.accounts.dividend_mint.to_account_info(),
        &ctx.accounts.dividend_vault.to_account_info(),
        &ctx.accounts.authority.to_account_info(),
        ctx.remaining_accounts,
        amount,
        ctx.accounts.dividend_mint.decimals,
        &[&seeds],
    )?;

    let landlord = &mut ctx.accounts.landlord;
    landlord.total_contributed = landlord
        .total_contributed
        .checked_add(amount)
        .ok_or(EndowmentError::Overflow)?;
    landlord.last_sweep_at = now;

    let config = &mut ctx.accounts.config;
    config.total_swept = config
        .total_swept
        .checked_add(amount)
        .ok_or(EndowmentError::Overflow)?;

    emit!(Swept {
        config: config_key,
        owner: landlord.owner,
        amount,
        total_contributed: landlord.total_contributed,
    });
    Ok(())
}
