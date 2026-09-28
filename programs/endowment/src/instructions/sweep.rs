use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::*,
    error::EndowmentError,
    events::Swept,
    state::{Config, Landlord},
};

/// Permissionless: anyone may crank a sweep. Funds can only move from the
/// landlord's delegated PUMP account into the endowment's PUMP vault.
#[derive(Accounts)]
pub struct Sweep<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: PDA that signs as the delegate.
    #[account(seeds = [AUTHORITY_SEED], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(mut, seeds = [LANDLORD_SEED, landlord.owner.as_ref()], bump = landlord.bump)]
    pub landlord: Account<'info, Landlord>,

    #[account(address = config.pump_mint)]
    pub pump_mint: InterfaceAccount<'info, Mint>,
    #[account(mut, address = landlord.pump_account)]
    pub pump_account: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        associated_token::mint = pump_mint,
        associated_token::authority = authority,
        associated_token::token_program = pump_token_program,
    )]
    pub pump_vault: InterfaceAccount<'info, TokenAccount>,

    pub pump_token_program: Interface<'info, TokenInterface>,
}

pub fn handle_sweep(ctx: Context<Sweep>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require!(!ctx.accounts.config.is_paused(now), EndowmentError::Paused);

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
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.pump_token_program.key(),
            TransferChecked {
                from: ctx.accounts.pump_account.to_account_info(),
                mint: ctx.accounts.pump_mint.to_account_info(),
                to: ctx.accounts.pump_vault.to_account_info(),
                authority: ctx.accounts.authority.to_account_info(),
            },
            signer,
        ),
        amount,
        ctx.accounts.pump_mint.decimals,
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
