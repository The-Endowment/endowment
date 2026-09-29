use anchor_lang::prelude::*;
use anchor_spl::token_interface::{TokenAccount, TokenInterface};

use crate::{constants::*, error::EndowmentError, events::ContributionsClosed, state::Config};

/// Permissionless check: closes landlord contributions for good once the
/// endowment's coin vault holds the contribution cap.
#[derive(Accounts)]
pub struct CloseContributions<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,
    /// CHECK: this endowment's authority PDA, which owns the vault.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        associated_token::mint = config.coin_mint,
        associated_token::authority = authority,
        associated_token::token_program = coin_token_program,
    )]
    pub coin_vault: InterfaceAccount<'info, TokenAccount>,
    pub coin_token_program: Interface<'info, TokenInterface>,
}

pub fn handle_close_contributions(ctx: Context<CloseContributions>) -> Result<()> {
    let held = ctx.accounts.coin_vault.amount;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    if config.closed {
        return Ok(());
    }
    require!(held >= config.contribution_cap, EndowmentError::CapNotReached);
    config.closed = true;
    emit!(ContributionsClosed { config: config_key, by_cap: true, coin_held: held });
    Ok(())
}
