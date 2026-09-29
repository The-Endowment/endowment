use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    constants::*,
    error::EndowmentError,
    events::EndowmentCreated,
    raydium::{PoolView, CPMM_PROGRAM_ID},
    state::{validate_activation, validate_buy_params, validate_donation, validate_limits, CommitmentCount, Config, CreateParams},
};

/// Permissionless: anyone can create an endowment for any coin that trades
/// against its dividend asset in a Raydium CPMM pool.
///
/// The instance's address includes the creator, so nobody can occupy or
/// front-run another creator's endowment: a squatter only ever creates a
/// separate instance of their own. The vaults are created with
/// `init_if_needed`, so someone pre-creating the (predictable) vault token
/// accounts can't block creation either.
#[derive(Accounts)]
pub struct CreateEndowment<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        init,
        payer = creator,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED, coin_mint.key().as_ref(), creator.key().as_ref()],
        bump
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: PDA that owns the instance's vaults and receives landlord delegations. Holds no data.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump)]
    pub authority: UncheckedAccount<'info>,

    #[account(mint::token_program = coin_token_program)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mint::token_program = dividend_token_program)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Receives landlord sweeps and the endowment's own dividends.
    #[account(
        init_if_needed,
        payer = creator,
        associated_token::mint = dividend_mint,
        associated_token::authority = authority,
        associated_token::token_program = dividend_token_program,
    )]
    pub dividend_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// Holds the bought coin. No instruction can move tokens out of it.
    #[account(
        init_if_needed,
        payer = creator,
        associated_token::mint = coin_mint,
        associated_token::authority = authority,
        associated_token::token_program = coin_token_program,
    )]
    pub coin_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: must be a Raydium CPMM pool holding exactly these two mints; parsed in the handler.
    #[account(owner = CPMM_PROGRAM_ID @ EndowmentError::InvalidPoolData)]
    pub pool_state: UncheckedAccount<'info>,

    pub coin_token_program: Interface<'info, TokenInterface>,
    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn handle_create_endowment(ctx: Context<CreateEndowment>, params: CreateParams) -> Result<()> {
    let coin_mint = ctx.accounts.coin_mint.key();
    let dividend_mint = ctx.accounts.dividend_mint.key();
    require_keys_neq!(coin_mint, dividend_mint, EndowmentError::SameMint);

    // The pool must trade exactly this coin against exactly this dividend asset,
    // in either order.
    let pool = PoolView::parse(&ctx.accounts.pool_state.try_borrow_data()?)?;
    let coin_index = pool.index_of(&coin_mint)?;
    let dividend_index = pool.index_of(&dividend_mint)?;
    require!(coin_index != dividend_index, EndowmentError::WrongPool);

    validate_limits(params.max_buy_per_tx, params.max_buy_per_day, params.max_price_impact_bps)?;
    validate_activation(params.activate_bps, params.deactivate_bps)?;
    validate_buy_params(params.buy_bps, params.min_buy_interval_secs, params.tip_bps)?;
    validate_donation(params.donation_bps, &dividend_mint, &ctx.accounts.config.key())?;
    require!(params.contribution_cap > 0, EndowmentError::InvalidContributionCap);
    require!(
        params.tip_bps + params.donation_bps <= MAX_TIP_PLUS_DONATION_BPS,
        EndowmentError::InvalidBuyParams
    );

    let creator = ctx.accounts.creator.key();
    let or_creator = |key: Pubkey| if key == Pubkey::default() { creator } else { key };

    ctx.accounts.config.set_inner(Config {
        creator,
        admin: or_creator(params.admin),
        pending_admin: Pubkey::default(),
        guardian: or_creator(params.guardian),
        coin_mint,
        dividend_mint,
        paused_until: 0,
        total_swept: 0,
        landlord_count: 0,
        bump: ctx.bumps.config,
        authority_bump: ctx.bumps.authority,
        pool: ctx.accounts.pool_state.key(),
        max_buy_per_tx: params.max_buy_per_tx,
        max_buy_per_day: params.max_buy_per_day,
        max_price_impact_bps: params.max_price_impact_bps,
        day_start: 0,
        bought_today: 0,
        total_dividend_spent: 0,
        total_coin_bought: 0,
        min_buy_interval_secs: params.min_buy_interval_secs,
        last_buy_at: 0,
        tip_bps: params.tip_bps,
        total_tips: 0,
        donation_bps: params.donation_bps,
        total_donated: 0,
        activate_bps: params.activate_bps,
        deactivate_bps: params.deactivate_bps,
        active: false,
        count: CommitmentCount::default(),
        contribution_cap: params.contribution_cap,
        closed: false,
        buy_bps: params.buy_bps,
        total_liquidity_dividend: 0,
        total_lp_tokens: 0,
    });
    // An activation threshold of 0 means sweeps run from the start.
    let config = &mut ctx.accounts.config;
    config.apply_committed_bps(0);

    emit!(EndowmentCreated {
        config: config.key(),
        creator,
        coin_mint,
        dividend_mint,
        pool: config.pool,
        donation_bps: config.donation_bps,
    });
    Ok(())
}
