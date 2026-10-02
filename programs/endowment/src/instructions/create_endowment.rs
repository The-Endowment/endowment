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
    state::{Config, CountRound, CreateParams, PendingParams},
    transfer::capped_transfer_fee_bps,
};

/// Creates the endowment for a coin that trades against its dividend asset in a
/// Raydium CPMM pool. Only FLAGSHIP_CREATOR can call it: this program runs the
/// $PENIS endowment only (other projects deploy their own copy).
///
/// The vaults are created with `init_if_needed`, so someone pre-creating the
/// (predictable) vault token accounts can't block creation.
#[derive(Accounts)]
pub struct CreateEndowment<'info> {
    #[account(
        mut,
        constraint = flagship_is_set() && creator.key() == FLAGSHIP_CREATOR @ EndowmentError::NotFlagshipCreator,
    )]
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

    /// Receives landlord sweeps.
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

pub fn handle_create_endowment(ctx: Context<CreateEndowment>, create: CreateParams) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let coin_mint = ctx.accounts.coin_mint.key();
    let dividend_mint = ctx.accounts.dividend_mint.key();
    require_keys_neq!(coin_mint, dividend_mint, EndowmentError::SameMint);
    // A fee already above the cap would give an endowment that can never trade.
    capped_transfer_fee_bps(&ctx.accounts.coin_mint.to_account_info(), clock.epoch)?;
    capped_transfer_fee_bps(&ctx.accounts.dividend_mint.to_account_info(), clock.epoch)?;

    // The pool must trade exactly this coin against exactly this dividend asset,
    // in either order.
    let pool = PoolView::parse(&ctx.accounts.pool_state.try_borrow_data()?)?;
    let coin_index = pool.index_of(&coin_mint)?;
    let dividend_index = pool.index_of(&dividend_mint)?;
    require!(coin_index != dividend_index, EndowmentError::WrongPool);

    let mut params = create.params;
    // The refresher defaults to the creator, like the admin and guardian.
    if params.refresher == Pubkey::default() {
        params.refresher = ctx.accounts.creator.key();
    }
    params.validate()?;
    require!(create.contribution_cap > 0, EndowmentError::InvalidContributionCap);

    let creator = ctx.accounts.creator.key();
    let or_creator = |key: Pubkey| if key == Pubkey::default() { creator } else { key };
    let config_key = ctx.accounts.config.key();

    ctx.accounts.config.set_inner(Config {
        version: CONFIG_VERSION,
        creator,
        admin: or_creator(create.admin),
        pending_admin: Pubkey::default(),
        guardian: or_creator(create.guardian),
        coin_mint,
        dividend_mint,
        pool: ctx.accounts.pool_state.key(),
        bump: ctx.bumps.config,
        authority_bump: ctx.bumps.authority,
        params,
        pending: PendingParams::default(),
        contribution_cap: create.contribution_cap,
        paused_until: 0,
        retired: false,
        retire_at: 0,
        milestone_reached: false,
        active: false,
        last_count_at: 0,
        last_count_bps: 0,
        last_committed: 0,
        last_attested_at: 0,
        last_sweep_at: 0,
        landlord_count: 0,
        count: CountRound::default(),
        buy_allowance: params.max_buy_per_tx,
        allowance_updated_at: now,
        last_buy_at: 0,
        total_swept: 0,
        total_dividend_spent: 0,
        total_coin_bought: 0,
        total_coin_retained: 0,
        total_liquidity_dividend: 0,
        total_liquidity_coin: 0,
        total_lp_tokens: 0,
        total_tips: 0,
        refresher_epoch: 0,
        reward_index: 0,
        last_reward_total: 0,
        last_reward_post_at: 0,
        reward_marks: Default::default(),
        reserved: [0; 20],
    });
    // An activation threshold of 0 means sweeps run from the start (for a
    // founders-only test window; renouncing requires production thresholds).
    let config = &mut ctx.accounts.config;
    config.apply_committed_bps(0);

    emit!(EndowmentCreated {
        config: config_key,
        creator,
        coin_mint,
        dividend_mint,
        pool: config.pool,
    });
    Ok(())
}
