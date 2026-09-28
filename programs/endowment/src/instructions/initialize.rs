use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{constants::*, error::EndowmentError, program::Endowment, state::Config};

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
    pub config: Account<'info, Config>,
    /// CHECK: PDA that owns the vaults and receives landlord delegations. Holds no data.
    #[account(seeds = [AUTHORITY_SEED], bump)]
    pub authority: UncheckedAccount<'info>,

    #[account(mint::token_program = pump_token_program)]
    pub pump_mint: InterfaceAccount<'info, Mint>,
    #[account(mint::token_program = penis_token_program)]
    pub penis_mint: InterfaceAccount<'info, Mint>,

    /// Receives landlord sweeps and the endowment's own PUMP dividends.
    #[account(
        init,
        payer = payer,
        associated_token::mint = pump_mint,
        associated_token::authority = authority,
        associated_token::token_program = pump_token_program,
    )]
    pub pump_vault: InterfaceAccount<'info, TokenAccount>,
    /// Holds bought-back $PENIS. No instruction can move tokens out of it.
    #[account(
        init,
        payer = payer,
        associated_token::mint = penis_mint,
        associated_token::authority = authority,
        associated_token::token_program = penis_token_program,
    )]
    pub penis_vault: InterfaceAccount<'info, TokenAccount>,

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
    supply_target_bps: u16,
) -> Result<()> {
    require!(
        (MIN_SUPPLY_TARGET_BPS..=MAX_SUPPLY_TARGET_BPS).contains(&supply_target_bps),
        EndowmentError::SupplyTargetOutOfBounds
    );

    ctx.accounts.config.set_inner(Config {
        admin,
        pending_admin: Pubkey::default(),
        guardian,
        pump_mint: ctx.accounts.pump_mint.key(),
        penis_mint: ctx.accounts.penis_mint.key(),
        supply_target_bps,
        paused_until: 0,
        total_swept: 0,
        landlord_count: 0,
        bump: ctx.bumps.config,
        authority_bump: ctx.bumps.authority,
    });
    Ok(())
}
