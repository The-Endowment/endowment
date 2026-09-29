use anchor_lang::prelude::*;
use anchor_spl::token_interface::{TokenAccount, TokenInterface};

use crate::{constants::*, error::EndowmentError, events::ContributionsClosed, state::Config};

/// Permissionless check: closes landlord contributions for good once the
/// $PENIS vault holds the contribution cap.
#[derive(Accounts)]
pub struct CloseContributions<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: PDA that owns the vault.
    #[account(seeds = [AUTHORITY_SEED], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        associated_token::mint = config.penis_mint,
        associated_token::authority = authority,
        associated_token::token_program = penis_token_program,
    )]
    pub penis_vault: InterfaceAccount<'info, TokenAccount>,
    pub penis_token_program: Interface<'info, TokenInterface>,
}

pub fn handle_close_contributions(ctx: Context<CloseContributions>) -> Result<()> {
    let held = ctx.accounts.penis_vault.amount;
    let config = &mut ctx.accounts.config;
    if config.closed {
        return Ok(());
    }
    require!(held >= config.contribution_cap, EndowmentError::CapNotReached);
    config.closed = true;
    emit!(ContributionsClosed { by_cap: true, penis_held: held });
    Ok(())
}
