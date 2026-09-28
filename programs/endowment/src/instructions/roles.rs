use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::EndowmentError,
    events::{AdminAccepted, AdminProposed, BuybackLimitsChanged, GuardianChanged},
    state::{validate_limits, Config},
};

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = admin @ EndowmentError::NotAdmin,
    )]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    pub new_admin: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = config.pending_admin != Pubkey::default()
            && config.pending_admin == new_admin.key() @ EndowmentError::NotPendingAdmin,
    )]
    pub config: Account<'info, Config>,
}

/// Adjusts buyback limits within the hard-coded bounds.
pub fn handle_set_buyback_limits(
    ctx: Context<AdminOnly>,
    max_buy_per_tx: u64,
    max_buy_per_day: u64,
    max_price_impact_bps: u16,
) -> Result<()> {
    validate_limits(max_buy_per_tx, max_buy_per_day, max_price_impact_bps)?;
    let config = &mut ctx.accounts.config;
    config.max_buy_per_tx = max_buy_per_tx;
    config.max_buy_per_day = max_buy_per_day;
    config.max_price_impact_bps = max_price_impact_bps;
    emit!(BuybackLimitsChanged { max_buy_per_tx, max_buy_per_day, max_price_impact_bps });
    Ok(())
}

/// Immediate: the guardian can only pause, so rotating it is low risk.
pub fn handle_set_guardian(ctx: Context<AdminOnly>, new_guardian: Pubkey) -> Result<()> {
    ctx.accounts.config.guardian = new_guardian;
    emit!(GuardianChanged { guardian: new_guardian });
    Ok(())
}

/// Step one of an admin handover. Proposing `Pubkey::default()` cancels.
pub fn handle_propose_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
    ctx.accounts.config.pending_admin = new_admin;
    emit!(AdminProposed { pending_admin: new_admin });
    Ok(())
}

/// Step two: the proposed admin must sign, so a typo can't brick the role.
pub fn handle_accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.admin = config.pending_admin;
    config.pending_admin = Pubkey::default();
    emit!(AdminAccepted { admin: config.admin });
    Ok(())
}
