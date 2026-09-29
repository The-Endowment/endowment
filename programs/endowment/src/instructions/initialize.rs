use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    constants::*,
    error::EndowmentError,
    program::Endowment,
    state::{validate_limits, CommitmentCount, Config},
};

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        init,
        payer = payer,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: PDA that owns the vaults and receives landlord delegations. Holds no data.
    #[account(seeds = [AUTHORITY_SEED], bump)]
    pub authority: UncheckedAccount<'info>,

    #[account(mint::token_program = pump_token_program)]
    pub pump_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mint::token_program = penis_token_program)]
    pub penis_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Receives landlord sweeps and the endowment's own PUMP dividends.
    #[account(
        init,
        payer = payer,
        associated_token::mint = pump_mint,
        associated_token::authority = authority,
        associated_token::token_program = pump_token_program,
    )]
    pub pump_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Holds bought-back $PENIS. No instruction can move tokens out of it.
    #[account(
        init,
        payer = payer,
        associated_token::mint = penis_mint,
        associated_token::authority = authority,
        associated_token::token_program = penis_token_program,
    )]
    pub penis_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    // Only the upgrade authority may initialize, so nobody can front-run the
    // deploy with their own admin and guardian.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, Endowment>,
    #[account(
        constraint = program_data.upgrade_authority_address == Some(payer.key())
            @ EndowmentError::NotUpgradeAuthority
    )]
    pub program_data: Account<'info, ProgramData>,

    pub pump_token_program: Interface<'info, TokenInterface>,
    pub penis_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn handle_initialize(
    ctx: Context<Initialize>,
    admin: Pubkey,
    guardian: Pubkey,
    pool: Pubkey,
    max_buy_per_tx: u64,
    max_buy_per_day: u64,
    max_price_impact_bps: u16,
) -> Result<()> {
    validate_limits(max_buy_per_tx, max_buy_per_day, max_price_impact_bps)?;

    ctx.accounts.config.set_inner(Config {
        admin,
        pending_admin: Pubkey::default(),
        guardian,
        pump_mint: ctx.accounts.pump_mint.key(),
        penis_mint: ctx.accounts.penis_mint.key(),
        paused_until: 0,
        total_swept: 0,
        landlord_count: 0,
        bump: ctx.bumps.config,
        authority_bump: ctx.bumps.authority,
        // The pool is only trusted after `buyback` checks that it is owned by
        // Raydium CPMM and holds exactly these two mints.
        pool,
        max_buy_per_tx,
        max_buy_per_day,
        max_price_impact_bps,
        day_start: 0,
        bought_today: 0,
        total_pump_spent: 0,
        total_penis_bought: 0,
        min_buy_interval_secs: DEFAULT_MIN_BUY_INTERVAL_SECS,
        last_buy_at: 0,
        tip_bps: DEFAULT_TIP_BPS,
        total_tips: 0,
        activate_bps: DEFAULT_ACTIVATE_BPS,
        deactivate_bps: DEFAULT_DEACTIVATE_BPS,
        active: false,
        count: CommitmentCount::default(),
        contribution_cap: CONTRIBUTION_CAP,
        closed: false,
        buy_bps: DEFAULT_BUY_BPS,
        total_liquidity_pump: 0,
        total_lp_tokens: 0,
    });
    Ok(())
}
