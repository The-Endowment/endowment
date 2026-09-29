use anchor_lang::{
    prelude::*,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        program::invoke_signed,
    },
};
use anchor_spl::{
    associated_token::get_associated_token_address_with_program_id,
    token::Token,
    token_2022::spl_token_2022::{
        extension::{transfer_fee::TransferFeeConfig, BaseStateWithExtensions, StateWithExtensions},
        state::Mint as MintState,
    },
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    constants::*,
    error::EndowmentError,
    events::{Bought, ContributionsClosed},
    math::{buy_amount, lp_tokens_for, min_acceptable_out, roll_day, split_buy},
    raydium::{
        pool_fee_bps, PoolView, CPMM_AUTH_SEED, CPMM_PROGRAM_ID, DEPOSIT_DISCRIMINATOR,
        SWAP_BASE_INPUT_DISCRIMINATOR,
    },
    state::Config,
    transfer::transfer_checked_with_hook,
};

/// Permissionless: anyone may crank a buyback, and is paid a small PUMP tip
/// for it. The contract decides the size. PUMP can only leave the PUMP vault
/// through the endowment's own Raydium pool (or as the capped tip), and $PENIS
/// can only land in the $PENIS vault. After contributions close, part of each
/// buyback becomes liquidity whose LP tokens land in an authority-owned
/// account that nothing can withdraw from.
///
/// Remaining accounts: PUMP transfer-hook extras for the tip, only if PUMP's
/// hook is ever switched on (see `transfer.rs`).
#[derive(Accounts)]
pub struct Buyback<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: PDA that owns every vault and signs the swap and deposit.
    #[account(seeds = [AUTHORITY_SEED], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,

    /// Whoever cranks the buyback; receives the tip.
    pub caller: Signer<'info>,
    /// The caller's PUMP token account, for the tip.
    #[account(
        mut,
        token::mint = pump_mint,
        token::authority = caller,
        token::token_program = pump_token_program,
    )]
    pub caller_pump_account: Box<InterfaceAccount<'info, TokenAccount>>,

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
    /// CHECK: the pool's LP mint; checked against the pool in the handler.
    #[account(mut)]
    pub lp_mint: UncheckedAccount<'info>,
    /// CHECK: the authority's LP token account (its associated token account
    /// for the LP mint); checked in the handler. Only used after contributions
    /// close. No instruction can move tokens out of it.
    #[account(mut)]
    pub lp_vault: UncheckedAccount<'info>,

    pub pump_token_program: Interface<'info, TokenInterface>,
    pub penis_token_program: Interface<'info, TokenInterface>,
    /// Raydium LP mints use the original SPL Token program.
    pub lp_token_program: Program<'info, Token>,
}

pub fn handle_buyback<'info>(ctx: Context<'info, Buyback<'info>>, min_out: u64) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let accounts = &ctx.accounts;
    let config = &accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(
        config.last_buy_at == 0 || now.saturating_sub(config.last_buy_at) >= config.min_buy_interval_secs,
        EndowmentError::BuyTooSoon
    );

    // The contract sizes the buy.
    let (day_start, bought_today) = roll_day(config.day_start, config.bought_today, now);
    let left_today = config.max_buy_per_day.saturating_sub(bought_today);
    let amount_in = buy_amount(accounts.pump_vault.amount, config.tip_bps, config.max_buy_per_tx, left_today);
    require!(amount_in > 0, EndowmentError::NothingToBuy);
    let bought_today = bought_today.checked_add(amount_in).ok_or(EndowmentError::Overflow)?;

    // Every Raydium account must belong to the configured pool.
    let pool = PoolView::parse(&accounts.pool_state.try_borrow_data()?)?;
    let pump_index = pool.index_of(&config.pump_mint)?;
    let penis_index = pool.index_of(&config.penis_mint)?;
    require_keys_eq!(accounts.amm_config.key(), pool.amm_config, EndowmentError::WrongPool);
    require_keys_eq!(accounts.observation_state.key(), pool.observation, EndowmentError::WrongPool);
    require_keys_eq!(accounts.pool_pump_vault.key(), pool.vaults[pump_index], EndowmentError::WrongPool);
    require_keys_eq!(accounts.pool_penis_vault.key(), pool.vaults[penis_index], EndowmentError::WrongPool);

    // Contributions close as soon as the vault reaches the cap.
    let mut closing_now = false;
    let closed = config.closed || {
        closing_now = accounts.penis_vault.amount >= config.contribution_cap;
        closing_now
    };
    let (swap_amount, deposit_pump, lp_swap) = if closed { split_buy(amount_in, config.buy_bps) } else { (amount_in, 0, 0) };

    let transfer_fee = penis_transfer_fee_bps(&accounts.penis_mint.to_account_info(), clock.epoch)?;
    let penis_before = accounts.penis_vault.amount;
    let pump_before = accounts.pump_vault.amount;

    // 1. Swap PUMP for $PENIS, protected by the price floor.
    let mut received = 0;
    let mut floor = 0;
    if swap_amount > 0 {
        let reserve_in = accounts.pool_pump_vault.amount.saturating_sub(pool.reserved_fees[pump_index]);
        let reserve_out = accounts.pool_penis_vault.amount.saturating_sub(pool.reserved_fees[penis_index]);
        let pool_fee = pool_fee_bps(&accounts.amm_config.try_borrow_data()?, pool.creator_fee_enabled)?;
        floor = min_acceptable_out(
            swap_amount,
            reserve_in,
            reserve_out,
            pool_fee,
            transfer_fee,
            config.max_price_impact_bps as u64,
        )
        .ok_or(EndowmentError::PriceImpactTooHigh)?;

        swap_base_input(&ctx, swap_amount, floor.max(min_out))?;
        ctx.accounts.penis_vault.reload()?;
        received = ctx.accounts.penis_vault.amount.saturating_sub(penis_before);
        require!(received >= floor, EndowmentError::PriceImpactTooHigh);
        require!(received >= min_out, EndowmentError::SlippageExceeded);
    }

    // 2. After close: deposit the liquidity share into the pool, locking the LP tokens.
    let mut lp_tokens = 0;
    if deposit_pump > 0 && received > 0 {
        let penis_budget = (received as u128 * lp_swap as u128 / swap_amount as u128) as u64;
        lp_tokens = deposit_liquidity(&ctx, &pool, pump_index, penis_index, deposit_pump, penis_budget, transfer_fee)?;
    }

    // The endowment's $PENIS never shrinks.
    ctx.accounts.penis_vault.reload()?;
    let penis_after = ctx.accounts.penis_vault.amount;
    require!(penis_after >= penis_before, EndowmentError::VaultWouldShrink);

    // 3. Pay the caller's tip in PUMP.
    let tip = (amount_in as u128 * ctx.accounts.config.tip_bps as u128 / 10_000) as u64;
    if tip > 0 {
        let signer: &[&[&[u8]]] = &[&[AUTHORITY_SEED, &[ctx.accounts.config.authority_bump]]];
        transfer_checked_with_hook(
            &ctx.accounts.pump_token_program.to_account_info(),
            &ctx.accounts.pump_vault.to_account_info(),
            &ctx.accounts.pump_mint.to_account_info(),
            &ctx.accounts.caller_pump_account.to_account_info(),
            &ctx.accounts.authority.to_account_info(),
            ctx.remaining_accounts,
            tip,
            ctx.accounts.pump_mint.decimals,
            signer,
        )?;
    }
    ctx.accounts.pump_vault.reload()?;
    let pump_spent = pump_before.saturating_sub(ctx.accounts.pump_vault.amount).saturating_sub(tip);
    let liquidity_pump = pump_spent.saturating_sub(swap_amount);

    let config = &mut ctx.accounts.config;
    if closing_now && !config.closed {
        config.closed = true;
        emit!(ContributionsClosed { by_cap: true, penis_held: penis_before });
    }
    if !config.closed && penis_after >= config.contribution_cap {
        config.closed = true;
        emit!(ContributionsClosed { by_cap: true, penis_held: penis_after });
    }
    config.day_start = day_start;
    config.bought_today = bought_today;
    config.last_buy_at = now;
    config.total_pump_spent = config.total_pump_spent.checked_add(pump_spent).ok_or(EndowmentError::Overflow)?;
    config.total_penis_bought = config.total_penis_bought.checked_add(received).ok_or(EndowmentError::Overflow)?;
    config.total_tips = config.total_tips.checked_add(tip).ok_or(EndowmentError::Overflow)?;
    config.total_liquidity_pump = config
        .total_liquidity_pump
        .checked_add(liquidity_pump)
        .ok_or(EndowmentError::Overflow)?;
    config.total_lp_tokens = config.total_lp_tokens.checked_add(lp_tokens).ok_or(EndowmentError::Overflow)?;

    emit!(Bought {
        pump_in: amount_in,
        penis_out: received,
        min_acceptable: floor,
        liquidity_pump,
        lp_tokens,
        tip,
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

/// Deposits up to `pump` PUMP and `penis_budget` $PENIS into the pool at its
/// current ratio, with the LP tokens landing in the authority's LP account.
/// Returns the LP tokens minted (0 if the amounts are too small to mint any).
fn deposit_liquidity(
    ctx: &Context<Buyback>,
    pool_before_swap: &PoolView,
    pump_index: usize,
    penis_index: usize,
    pump: u64,
    penis_budget: u64,
    transfer_fee_bps: u64,
) -> Result<u64> {
    let a = &ctx.accounts;
    require_keys_eq!(a.lp_mint.key(), pool_before_swap.lp_mint, EndowmentError::WrongPool);
    let expected_lp_vault =
        get_associated_token_address_with_program_id(&a.authority.key(), &a.lp_mint.key(), &a.lp_token_program.key());
    require_keys_eq!(a.lp_vault.key(), expected_lp_vault, EndowmentError::WrongPool);

    // Reserves after our own swap, as Raydium will see them.
    let pool = PoolView::parse(&a.pool_state.try_borrow_data()?)?;
    let reserve_pump =
        token_amount(&a.pool_pump_vault.to_account_info())?.saturating_sub(pool.reserved_fees[pump_index]);
    let reserve_penis =
        token_amount(&a.pool_penis_vault.to_account_info())?.saturating_sub(pool.reserved_fees[penis_index]);

    // $PENIS pays its transfer fee on the way in; budget for what arrives.
    let penis_net = (penis_budget as u128 * (10_000 - transfer_fee_bps as u128) / 10_000) as u64;
    let lp_amount = lp_tokens_for(pump, penis_net.saturating_sub(1), reserve_pump, reserve_penis, pool.lp_supply);
    if lp_amount == 0 {
        return Ok(0);
    }

    let lp_before = token_amount(&a.lp_vault.to_account_info())?;
    let mut max = [0u64; 2];
    max[pump_index] = pump;
    max[penis_index] = penis_budget;
    let mut data = DEPOSIT_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&lp_amount.to_le_bytes());
    data.extend_from_slice(&max[0].to_le_bytes());
    data.extend_from_slice(&max[1].to_le_bytes());

    let our_account = |i: usize| if i == pump_index { a.pump_vault.to_account_info() } else { a.penis_vault.to_account_info() };
    let pool_vault = |i: usize| {
        if i == pump_index { a.pool_pump_vault.to_account_info() } else { a.pool_penis_vault.to_account_info() }
    };
    let mint = |i: usize| if i == pump_index { a.pump_mint.to_account_info() } else { a.penis_mint.to_account_info() };

    // Account order from the IDL: owner, authority, pool_state, owner_lp_token,
    // token_0_account, token_1_account, token_0_vault, token_1_vault,
    // token_program, token_program_2022, vault_0_mint, vault_1_mint, lp_mint.
    let infos = [
        a.authority.to_account_info(),
        a.cpmm_authority.to_account_info(),
        a.pool_state.to_account_info(),
        a.lp_vault.to_account_info(),
        our_account(0),
        our_account(1),
        pool_vault(0),
        pool_vault(1),
        a.lp_token_program.to_account_info(),
        a.penis_token_program.to_account_info(),
        mint(0),
        mint(1),
        a.lp_mint.to_account_info(),
    ];
    let writable = [false, false, true, true, true, true, true, true, false, false, false, false, true];
    let metas = infos
        .iter()
        .zip(writable)
        .enumerate()
        .map(|(i, (info, w))| {
            let signer = i == 0;
            if w { AccountMeta::new(info.key(), signer) } else { AccountMeta::new_readonly(info.key(), signer) }
        })
        .collect();
    let ix = Instruction { program_id: CPMM_PROGRAM_ID, accounts: metas, data };

    let mut all = infos.to_vec();
    all.push(a.cpmm_program.to_account_info());
    let signer: &[&[&[u8]]] = &[&[AUTHORITY_SEED, &[a.config.authority_bump]]];
    invoke_signed(&ix, &all, signer)?;

    let lp_after = token_amount(&a.lp_vault.to_account_info())?;
    Ok(lp_after.saturating_sub(lp_before))
}

/// Balance of an SPL token account (legacy or Token-2022), from its raw data.
fn token_amount(info: &AccountInfo) -> Result<u64> {
    let data = info.try_borrow_data()?;
    require!(data.len() >= 72, EndowmentError::WrongPool);
    Ok(u64::from_le_bytes(data[64..72].try_into().unwrap()))
}
