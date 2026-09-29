use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::EndowmentError,
    events::{
        ActivationChanged, AdminAccepted, AdminProposed, AdminRenounced, BuyParamsChanged, BuybackLimitsChanged,
        ContributionsClosed, GuardianChanged,
    },
    state::{validate_activation, validate_buy_params, validate_limits, Config},
};

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
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
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
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
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    config.max_buy_per_tx = max_buy_per_tx;
    config.max_buy_per_day = max_buy_per_day;
    config.max_price_impact_bps = max_price_impact_bps;
    emit!(BuybackLimitsChanged { config: config_key, max_buy_per_tx, max_buy_per_day, max_price_impact_bps });
    Ok(())
}

/// Adjusts the post-close buy/liquidity split, buyback pacing and the crank
/// tip, within the hard-coded bounds. The donation rate is locked at creation.
pub fn handle_set_buy_params(
    ctx: Context<AdminOnly>,
    buy_bps: u16,
    min_buy_interval_secs: i64,
    tip_bps: u16,
) -> Result<()> {
    validate_buy_params(buy_bps, min_buy_interval_secs, tip_bps)?;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(
        tip_bps + config.donation_bps <= MAX_TIP_PLUS_DONATION_BPS,
        EndowmentError::InvalidBuyParams
    );
    config.buy_bps = buy_bps;
    config.min_buy_interval_secs = min_buy_interval_secs;
    config.tip_bps = tip_bps;
    emit!(BuyParamsChanged { config: config_key, buy_bps, min_buy_interval_secs, tip_bps });
    Ok(())
}

/// Sets the activation thresholds and re-applies them to the last count, so
/// setting `activate_bps` to 0 turns sweeps on at once (for a founders-only test).
pub fn handle_set_activation(ctx: Context<AdminOnly>, activate_bps: u16, deactivate_bps: u16) -> Result<()> {
    validate_activation(activate_bps, deactivate_bps)?;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    config.activate_bps = activate_bps;
    config.deactivate_bps = deactivate_bps;
    let last = config.count.last_committed_bps;
    config.apply_committed_bps(last);
    emit!(ActivationChanged { config: config_key, activate_bps, deactivate_bps, active: config.active });
    Ok(())
}

/// One-way: closes landlord contributions early. It can never move funds.
pub fn handle_retire(ctx: Context<AdminOnly>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    if !config.closed {
        config.closed = true;
        emit!(ContributionsClosed { config: config_key, by_cap: false, coin_held: 0 });
    }
    Ok(())
}

/// One-way: gives up the admin role for good, freezing every bounded
/// parameter as it stands.
pub fn handle_renounce_admin(ctx: Context<AdminOnly>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    config.admin = Pubkey::default();
    config.pending_admin = Pubkey::default();
    emit!(AdminRenounced { config: config_key });
    Ok(())
}

/// Immediate: the guardian can only pause, so rotating it is low risk.
pub fn handle_set_guardian(ctx: Context<AdminOnly>, new_guardian: Pubkey) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    ctx.accounts.config.guardian = new_guardian;
    emit!(GuardianChanged { config: config_key, guardian: new_guardian });
    Ok(())
}

/// Step one of an admin handover. Proposing `Pubkey::default()` cancels.
pub fn handle_propose_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    ctx.accounts.config.pending_admin = new_admin;
    emit!(AdminProposed { config: config_key, pending_admin: new_admin });
    Ok(())
}

/// Step two: the proposed admin must sign, so a typo can't brick the role.
pub fn handle_accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    config.admin = config.pending_admin;
    config.pending_admin = Pubkey::default();
    emit!(AdminAccepted { config: config_key, admin: config.admin });
    Ok(())
}
