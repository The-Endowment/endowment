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
/// landlord's delegated PUMP account into the endowment's PUMP vault.
///
/// Remaining accounts: PUMP transfer-hook extras, only if PUMP's hook is ever
/// switched on (see `transfer.rs`).
#[derive(Accounts)]
pub struct Sweep<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: PDA that signs as the delegate.
    #[account(seeds = [AUTHORITY_SEED], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(mut, seeds = [LANDLORD_SEED, landlord.owner.as_ref()], bump = landlord.bump)]
    pub landlord: Box<Account<'info, Landlord>>,

    #[account(address = config.pump_mint)]
    pub pump_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, address = landlord.pump_account)]
    pub pump_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = pump_mint,
        associated_token::authority = authority,
        associated_token::token_program = pump_token_program,
    )]
    pub pump_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Read to close contributions as soon as the cap is reached.
    #[account(
        associated_token::mint = config.penis_mint,
        associated_token::authority = authority,
        associated_token::token_program = penis_token_program,
    )]
    pub penis_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub pump_token_program: Interface<'info, TokenInterface>,
    pub penis_token_program: Interface<'info, TokenInterface>,
}

pub fn handle_sweep<'info>(ctx: Context<'info, Sweep<'info>>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config = &mut ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);

    // Contributions close for good once the vault reaches the cap.
    if !config.closed && ctx.accounts.penis_vault.amount >= config.contribution_cap {
        config.closed = true;
        emit!(ContributionsClosed { by_cap: true, penis_held: ctx.accounts.penis_vault.amount });
        // Persist the close even though this sweep does nothing else.
        return Ok(());
    }
    require!(!config.closed, EndowmentError::ContributionsClosed);
    require!(config.active, EndowmentError::NotActive);

    let pump_account = &ctx.accounts.pump_account;
    require!(
        pump_account.delegate == Some(ctx.accounts.authority.key()).into(),
        EndowmentError::NotDelegated
    );

    let (amount, baseline) = ctx
        .accounts
        .landlord
        .sweepable(pump_account.amount, pump_account.delegated_amount);
    ctx.accounts.landlord.baseline = baseline;
    if amount == 0 {
        return Ok(());
    }

    let signer: &[&[&[u8]]] = &[&[AUTHORITY_SEED, &[ctx.accounts.config.authority_bump]]];
    transfer_checked_with_hook(
        &ctx.accounts.pump_token_program.to_account_info(),
        &ctx.accounts.pump_account.to_account_info(),
        &ctx.accounts.pump_mint.to_account_info(),
        &ctx.accounts.pump_vault.to_account_info(),
        &ctx.accounts.authority.to_account_info(),
        ctx.remaining_accounts,
        amount,
        ctx.accounts.pump_mint.decimals,
        signer,
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
        owner: landlord.owner,
        amount,
        total_contributed: landlord.total_contributed,
    });
    Ok(())
}
