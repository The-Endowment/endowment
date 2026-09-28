use anchor_lang::{
    prelude::*,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        program::invoke_signed,
    },
};
use anchor_spl::{
    token_2022::spl_token_2022::{
        extension::{transfer_fee::TransferFeeConfig, BaseStateWithExtensions, StateWithExtensions},
        state::Mint as MintState,
    },
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    constants::*,
    error::EndowmentError,
    events::Bought,
    math::{min_acceptable_out, roll_day},
    raydium::{pool_fee_bps, PoolView, CPMM_AUTH_SEED, CPMM_PROGRAM_ID, SWAP_BASE_INPUT_DISCRIMINATOR},
    state::Config,
};

/// Permissionless: anyone may crank a buyback. PUMP can only leave the PUMP
/// vault through the endowment's own Raydium pool, and the $PENIS it buys can
/// only land in the $PENIS vault.
#[derive(Accounts)]
pub struct Buyback<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: PDA that owns both vaults and signs the swap.
    #[account(seeds = [AUTHORITY_SEED], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,

    #[account(address = config.pump_mint)]
    pub pump_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = config.penis_mint)]
    pub penis_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        associated_token::mint = pump_mint,
        associated_token::authority = authority,
        associated_token::token_program = pump_token_program,
    )]
    pub pump_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = penis_mint,
        associated_token::authority = authority,
        associated_token::token_program = penis_token_program,
    )]
    pub penis_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: the Raydium CPMM program.
    #[account(address = CPMM_PROGRAM_ID)]
    pub cpmm_program: UncheckedAccount<'info>,
    /// CHECK: Raydium's vault authority PDA; checked against its seed.
    #[account(seeds = [CPMM_AUTH_SEED], bump, seeds::program = CPMM_PROGRAM_ID)]
    pub cpmm_authority: UncheckedAccount<'info>,
    /// CHECK: must be the pool's AMM config; checked in the handler.
    #[account(owner = CPMM_PROGRAM_ID)]
    pub amm_config: UncheckedAccount<'info>,
    /// CHECK: the pool stored in config, owned by Raydium CPMM; parsed in the handler.
    #[account(mut, address = config.pool @ EndowmentError::WrongPool, owner = CPMM_PROGRAM_ID)]
    pub pool_state: UncheckedAccount<'info>,
    /// The pool's PUMP vault; checked against the pool in the handler.
    #[account(mut)]
    pub pool_pump_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// The pool's $PENIS vault; checked against the pool in the handler.
    #[account(mut)]
    pub pool_penis_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the pool's observation account; checked in the handler.
    #[account(mut, owner = CPMM_PROGRAM_ID)]
    pub observation_state: UncheckedAccount<'info>,

    pub pump_token_program: Interface<'info, TokenInterface>,
    pub penis_token_program: Interface<'info, TokenInterface>,
}

pub fn handle_buyback(ctx: Context<Buyback>, amount_in: u64, min_out: u64) -> Result<()> {
    let clock = Clock::get()?;
    let config = &ctx.accounts.config;
    require!(!config.is_paused(clock.unix_timestamp), EndowmentError::Paused);
    require!(amount_in > 0, EndowmentError::ZeroAmount);
    require!(amount_in <= config.max_buy_per_tx, EndowmentError::BuyTooLarge);
    let (day_start, bought_today) = roll_day(config.day_start, config.bought_today, clock.unix_timestamp);
    let bought_today = bought_today.checked_add(amount_in).ok_or(EndowmentError::Overflow)?;
    require!(bought_today <= config.max_buy_per_day, EndowmentError::DailyCapReached);

    // Every Raydium account must belong to the configured pool.
    let pool = PoolView::parse(&ctx.accounts.pool_state.try_borrow_data()?)?;
    let pump_index = pool.index_of(&config.pump_mint)?;
    let penis_index = pool.index_of(&config.penis_mint)?;
    let accounts = &ctx.accounts;
    require_keys_eq!(accounts.amm_config.key(), pool.amm_config, EndowmentError::WrongPool);
    require_keys_eq!(accounts.observation_state.key(), pool.observation, EndowmentError::WrongPool);
    require_keys_eq!(accounts.pool_pump_vault.key(), pool.vaults[pump_index], EndowmentError::WrongPool);
    require_keys_eq!(accounts.pool_penis_vault.key(), pool.vaults[penis_index], EndowmentError::WrongPool);

    // The price floor, from reserves and fees as they stand before the trade.
    let reserve_in = accounts.pool_pump_vault.amount.saturating_sub(pool.reserved_fees[pump_index]);
    let reserve_out = accounts.pool_penis_vault.amount.saturating_sub(pool.reserved_fees[penis_index]);
    let pool_fee = pool_fee_bps(&accounts.amm_config.try_borrow_data()?, pool.creator_fee_enabled)?;
    let transfer_fee = penis_transfer_fee_bps(&accounts.penis_mint.to_account_info(), clock.epoch)?;
    let floor = min_acceptable_out(
        amount_in,
        reserve_in,
        reserve_out,
        pool_fee,
        transfer_fee,
        config.max_price_impact_bps as u64,
    )
    .ok_or(EndowmentError::PriceImpactTooHigh)?;

    let penis_before = accounts.penis_vault.amount;
    swap_base_input(&ctx, amount_in, floor.max(min_out))?;
    ctx.accounts.penis_vault.reload()?;
    let received = ctx.accounts.penis_vault.amount.saturating_sub(penis_before);
    require!(received >= floor, EndowmentError::PriceImpactTooHigh);
    require!(received >= min_out, EndowmentError::SlippageExceeded);

    let config = &mut ctx.accounts.config;
    config.day_start = day_start;
    config.bought_today = bought_today;
    config.total_pump_spent = config.total_pump_spent.checked_add(amount_in).ok_or(EndowmentError::Overflow)?;
    config.total_penis_bought = config.total_penis_bought.checked_add(received).ok_or(EndowmentError::Overflow)?;

    emit!(Bought {
        pump_in: amount_in,
        penis_out: received,
        min_acceptable: floor,
        total_pump_spent: config.total_pump_spent,
        total_penis_bought: config.total_penis_bought,
    });
    Ok(())
}

/// The $PENIS transfer fee for this epoch, in basis points (0 if none).
fn penis_transfer_fee_bps(mint: &AccountInfo, epoch: u64) -> Result<u64> {
    let data = mint.try_borrow_data()?;
    let state = StateWithExtensions::<MintState>::unpack(&data)?;
    Ok(match state.get_extension::<TransferFeeConfig>() {
        Ok(config) => u16::from(config.get_epoch_fee(epoch).transfer_fee_basis_points) as u64,
        Err(_) => 0,
    })
}

/// CPI into Raydium CPMM `swap_base_input`, signed by the endowment's authority.
fn swap_base_input(ctx: &Context<Buyback>, amount_in: u64, minimum_amount_out: u64) -> Result<()> {
    let a = &ctx.accounts;
    let mut data = SWAP_BASE_INPUT_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    // Account order from the IDL: payer, authority, amm_config, pool_state,
    // input/output token accounts, input/output vaults, input/output token
    // programs, input/output mints, observation_state.
    let ix = Instruction {
        program_id: CPMM_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(a.authority.key(), true),
            AccountMeta::new_readonly(a.cpmm_authority.key(), false),
            AccountMeta::new_readonly(a.amm_config.key(), false),
            AccountMeta::new(a.pool_state.key(), false),
            AccountMeta::new(a.pump_vault.key(), false),
            AccountMeta::new(a.penis_vault.key(), false),
            AccountMeta::new(a.pool_pump_vault.key(), false),
            AccountMeta::new(a.pool_penis_vault.key(), false),
            AccountMeta::new_readonly(a.pump_token_program.key(), false),
            AccountMeta::new_readonly(a.penis_token_program.key(), false),
            AccountMeta::new_readonly(a.pump_mint.key(), false),
            AccountMeta::new_readonly(a.penis_mint.key(), false),
            AccountMeta::new(a.observation_state.key(), false),
        ],
        data,
    };
    let signer: &[&[&[u8]]] = &[&[AUTHORITY_SEED, &[a.config.authority_bump]]];
    invoke_signed(
        &ix,
        &[
            a.authority.to_account_info(),
            a.cpmm_authority.to_account_info(),
            a.amm_config.to_account_info(),
            a.pool_state.to_account_info(),
            a.pump_vault.to_account_info(),
            a.penis_vault.to_account_info(),
            a.pool_pump_vault.to_account_info(),
            a.pool_penis_vault.to_account_info(),
            a.pump_token_program.to_account_info(),
            a.penis_token_program.to_account_info(),
            a.pump_mint.to_account_info(),
            a.penis_mint.to_account_info(),
            a.observation_state.to_account_info(),
            a.cpmm_program.to_account_info(),
        ],
        signer,
    )?;
    Ok(())
}
