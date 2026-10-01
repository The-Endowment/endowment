use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    constants::*,
    error::EndowmentError,
    events::LandlordRegistered,
    state::{Config, Landlord},
};

/// Owner-signed consent to reporter-authorized reward collection.
/// SPL delegation remains revocable directly through the token program.
#[derive(Accounts)]
pub struct RegisterLandlord<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: this endowment's authority PDA; address check only.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        init,
        payer = owner,
        space = 8 + Landlord::INIT_SPACE,
        seeds = [LANDLORD_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump
    )]
    pub landlord: Box<Account<'info, Landlord>>,

    #[account(address = config.dividend_mint)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        associated_token::mint = dividend_mint,
        associated_token::authority = owner,
        associated_token::token_program = dividend_token_program,
        constraint = dividend_account.delegate == Some(authority.key()).into()
            && dividend_account.delegated_amount >= MIN_DELEGATION
            @ EndowmentError::NotDelegated,
    )]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    /// Counted toward activation in each daily count.
    #[account(
        associated_token::mint = coin_mint,
        associated_token::authority = owner,
        associated_token::token_program = coin_token_program,
    )]
    pub coin_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub coin_token_program: Interface<'info, TokenInterface>,
    #[account(seeds = [REPORTER_SEED, config.key().as_ref()], bump = reporter_policy.bump,
        has_one = config, constraint = !reporter_policy.disabled @ EndowmentError::NotReporter)]
    pub reporter_policy: Box<Account<'info, crate::reports::ReporterPolicy>>,
    pub system_program: Program<'info, System>,
}

pub fn handle_enroll_rewards(ctx: Context<RegisterLandlord>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let config = &mut ctx.accounts.config;
    require!(config.version == CONFIG_VERSION, EndowmentError::UnsupportedCollectionVersion);
    let consent_id = config.new_consent()?;
    require!(!config.retired, EndowmentError::Retired);
    require!(!config.milestone_reached, EndowmentError::Completed);
    require!(!config.is_paused(now), EndowmentError::Paused);

    let coin_held = ctx.accounts.coin_account.amount;
    require!(coin_held >= config.min_stake(ctx.accounts.coin_mint.supply), EndowmentError::StakeTooSmall);
    config.landlord_count = config.landlord_count.checked_add(1).ok_or(EndowmentError::Overflow)?;
    // Counted from the next round on: a round already open doesn't expect it.
    let joined_round = config.count.round;

    let owner = ctx.accounts.owner.key();
    let baseline = ctx.accounts.dividend_account.amount;
    ctx.accounts.landlord.set_inner(Landlord {
        version: LANDLORD_VERSION,
        config: config_key,
        owner,
        dividend_account: ctx.accounts.dividend_account.key(),
        coin_account: ctx.accounts.coin_account.key(),
        baseline,
        total_contributed: 0,
        registered_at: now,
        last_sweep_at: 0,
        bump: ctx.bumps.landlord,
        joined_round,
        counted_round: 0,
        counted_amount: 0,
        snapshot: 0,
        snapshot_valid: false,
        attestations: 0,
        last_attested_at: 0,
        attestation_epoch: 0,
        consent_id,
        last_report_nonce: 0,
        reserved: [0; 36],
    });

    emit!(crate::events::CollectionConsent { config: config_key, owner, consent_id });
    emit!(LandlordRegistered { config: config_key, owner, baseline, coin_held });
    Ok(())
}
