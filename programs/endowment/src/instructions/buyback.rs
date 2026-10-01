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
    token_2022::Token2022,
    token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked},
};

use crate::{
    constants::*,
    error::EndowmentError,
    events::Bought,
    health::{ensure_tradeable, is_frozen, TradeAccounts},
    math::{
        floor_slippage_bps, impact_cap, lp_tokens_for, min_acceptable_out, quote_out, refill, spendable, split_buy,
        spot_price_x32, zap_swap_amount,
    },
    raydium::{twap_price_x32, PoolView, CPMM_AUTH_SEED, CPMM_PROGRAM_ID, DEPOSIT_DISCRIMINATOR, SWAP_BASE_INPUT_DISCRIMINATOR},
    state::Config,
    transfer::{capped_transfer_fee_bps, hook_enabled, read_token_account},
};

/// Permissionless: anyone may crank a buyback for an endowment, and is paid a
/// small tip in the dividend asset for it. The contract decides the size, and
/// measures the price against the pool's time-weighted average, so nothing the
/// caller does in the same transaction can worsen the fill.
///
/// The dividend can only leave the dividend vault through the endowment's own
/// Raydium pool, as the capped tip, or as the donation locked in at creation;
/// the coin can only land in the endowment's coin vault. After the goal (the
/// coin vault holding `contribution_cap`, however it got there), `buy_bps` of
/// each buyback buys the coin and the rest becomes liquidity whose LP tokens
/// land in an authority-owned account that nothing can withdraw from.
///
/// Buybacks refuse to run while either mint has a transfer hook set (see
/// `health::ensure_tradeable`), so the tip and donation are plain transfers.
///
/// An endowment that donates passes the flagship's coin mint as the first
/// remaining account: the donation is skipped (kept in this vault) while the
/// flagship coin can't trade, so it doesn't pile up where it can't be spent.
#[derive(Accounts)]
pub struct Buyback<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: this endowment's authority PDA; owns every vault and signs the swap and deposit.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,

    /// Whoever cranks the buyback; receives the tip.
    pub caller: Signer<'info>,
    /// The caller's dividend token account, for the tip.
    #[account(
        mut,
        token::mint = dividend_mint,
        token::authority = caller,
        token::token_program = dividend_token_program,
    )]
    pub caller_dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = config.dividend_mint)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        associated_token::mint = dividend_mint,
        associated_token::authority = authority,
        associated_token::token_program = dividend_token_program,
    )]
    pub dividend_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = coin_mint,
        associated_token::authority = authority,
        associated_token::token_program = coin_token_program,
    )]
    pub coin_vault: Box<InterfaceAccount<'info, TokenAccount>>,

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
    /// The pool's dividend vault; checked against the pool in the handler.
    #[account(mut)]
    pub pool_dividend_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// The pool's coin vault; checked against the pool in the handler.
    #[account(mut)]
    pub pool_coin_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the pool's observation account (its price history); checked and read in the handler.
    #[account(mut, owner = CPMM_PROGRAM_ID)]
    pub observation_state: UncheckedAccount<'info>,
    /// CHECK: the pool's LP mint; checked against the pool in the handler.
    #[account(mut)]
    pub lp_mint: UncheckedAccount<'info>,
    /// CHECK: the authority's LP token account (its associated token account for
    /// the LP mint); checked in the handler. While it doesn't exist, the
    /// liquidity share is spent buying instead. No instruction can move tokens
    /// out of it.
    #[account(mut)]
    pub lp_vault: UncheckedAccount<'info>,
    /// CHECK: the flagship endowment's dividend vault. Only touched, and then
    /// checked against its derived address (and required writable), when this
    /// endowment donates; read-only otherwise, so buybacks of endowments that
    /// don't donate don't contend for it.
    pub flagship_dividend_vault: UncheckedAccount<'info>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub coin_token_program: Interface<'info, TokenInterface>,
    /// Raydium LP mints use the original SPL Token program.
    pub lp_token_program: Program<'info, Token>,
    /// Raydium's deposit takes both token programs explicitly.
    pub token_2022_program: Program<'info, Token2022>,
}

/// Everything the buy is priced and sized from, read before any CPI.
struct Plan {
    dividend_index: usize,
    coin_index: usize,
    twap_price_x32: u128,
    /// The TWAP reader's bound on its own error for this history, in bps.
    twap_error_bps: u64,
    pool_fee: u64,
    dividend_fee: u64,
    coin_fee: u64,
    allowance: u64,
    /// Dividend to swap for the coin (the buy share plus the liquidity share's swap half).
    swap_amount: u64,
    /// What the pool would pay for `swap_amount` now, net of every fee.
    quote: u64,
    /// Of `swap_amount`, the part swapped for the liquidity deposit.
    lp_swap: u64,
    /// Dividend to deposit as liquidity alongside the coin from `lp_swap`.
    lp_deposit: u64,
}

pub fn handle_buyback<'info>(ctx: Context<'info, Buyback<'info>>, min_out: u64) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    // Coin sent to the vault directly counts toward the goal before the buy/
    // liquidity split is chosen.
    let coin_held = ctx.accounts.coin_vault.amount;
    ctx.accounts.config.record_completion(config_key, coin_held);
    let plan = plan_buy(&ctx.accounts, clock.epoch, now)?;

    let a = &ctx.accounts;
    let coin_before = a.coin_vault.amount;
    let dividend_before = a.dividend_vault.amount;
    let lp_before = read_token_account(&a.lp_vault)?.map(|t| t.amount).unwrap_or(0);

    // 1. Swap the dividend for the coin, protected by the TWAP floor.
    let params = &a.config.params;
    let floor = min_acceptable_out(
        plan.swap_amount,
        plan.twap_price_x32,
        plan.pool_fee,
        plan.dividend_fee,
        plan.coin_fee,
        floor_slippage_bps(params.max_price_impact_bps, params.max_twap_deviation_bps, plan.twap_error_bps),
    )
    .ok_or(EndowmentError::PriceImpactTooHigh)?;
    require!(floor > 0, EndowmentError::PriceImpactTooHigh);
    // The spot band passed, but the pool's own quote for this size is still
    // under the floor: say so, rather than leave it to Raydium's slippage error.
    require!(plan.quote >= floor, EndowmentError::FloorAboveQuote);
    swap_base_input(&ctx, &config_key, plan.swap_amount, floor.max(min_out))?;
    ctx.accounts.coin_vault.reload()?;
    let received = ctx.accounts.coin_vault.amount.saturating_sub(coin_before);
    require!(received >= floor, EndowmentError::PriceImpactTooHigh);
    require!(received >= min_out, EndowmentError::SlippageExceeded);

    // 2. After the milestone: deposit the liquidity share, locking the LP tokens.
    if plan.lp_deposit > 0 && plan.lp_swap > 0 {
        let coin_budget = (received as u128 * plan.lp_swap as u128 / plan.swap_amount as u128) as u64;
        deposit_liquidity(&ctx, &config_key, &plan, coin_budget)?;
    }

    // 3. Nothing Raydium did may shrink the coin vault or the LP vault, take
    //    more dividend than planned, or leave a delegate or close authority behind.
    ctx.accounts.coin_vault.reload()?;
    ctx.accounts.dividend_vault.reload()?;
    let a = &ctx.accounts;
    let coin_after = a.coin_vault.amount;
    let dividend_after = a.dividend_vault.amount;
    require!(coin_after >= coin_before, EndowmentError::VaultWouldShrink);
    require!(dividend_after <= dividend_before, EndowmentError::CpiInvariant);
    let spent = dividend_before - dividend_after;
    require!(spent <= plan.swap_amount + plan.lp_deposit, EndowmentError::CpiInvariant);
    let lp_view = read_token_account(&a.lp_vault)?;
    let lp_after = lp_view.as_ref().map(|t| t.amount).unwrap_or(0);
    require!(lp_after >= lp_before, EndowmentError::CpiInvariant);
    let authority = a.authority.key();
    for vault in [a.dividend_vault.to_account_info(), a.coin_vault.to_account_info()] {
        let view = read_token_account(&vault)?.ok_or(EndowmentError::CpiInvariant)?;
        require!(
            view.owner == authority && view.delegate.is_none() && view.close_authority.is_none(),
            EndowmentError::CpiInvariant
        );
    }
    if let Some(view) = lp_view {
        require!(
            view.owner == authority && view.delegate.is_none() && view.close_authority.is_none(),
            EndowmentError::CpiInvariant
        );
    }

    let bump = [a.config.authority_bump];
    let seeds = Config::authority_seeds(&config_key, &bump);

    let pay = |to: AccountInfo<'info>, amount: u64| {
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                a.dividend_token_program.key(),
                TransferChecked {
                    from: a.dividend_vault.to_account_info(),
                    mint: a.dividend_mint.to_account_info(),
                    to,
                    authority: a.authority.to_account_info(),
                },
                &[&seeds],
            ),
            amount,
            a.dividend_mint.decimals,
        )
    };

    // 4. Tip the caller, on what was actually spent.
    let tip = (spent as u128 * a.config.params.tip_bps as u128 / 10_000) as u64;
    if tip > 0 {
        pay(a.caller_dividend_account.to_account_info(), tip)?;
    }

    // 5. Send the donation, if this endowment chose one, to the flagship's
    //    dividend vault, while it exists, isn't frozen, the flagship coin can
    //    trade, and only up to the flagship's own vault cap: a donation never
    //    takes the flagship vault above what it can spend in
    //    MAX_VAULT_DAYS_OF_BUYS days, like a sweep (R3-MINT-01). The donor's
    //    buybacks never depend on the flagship's state.
    let mut donation = (spent as u128 * a.config.donation_bps as u128 / 10_000) as u64;
    let mut flagship_cap = 0u64;
    if donation > 0 {
        match flagship_state(ctx.remaining_accounts, clock.epoch, ctx.program_id)? {
            Some(cap) => flagship_cap = cap,
            None => donation = 0,
        }
    }
    if donation > 0 {
        let (flagship_authority, _) =
            Pubkey::find_program_address(&[AUTHORITY_SEED, flagship_config().as_ref()], ctx.program_id);
        let expected = get_associated_token_address_with_program_id(
            &flagship_authority,
            &a.dividend_mint.key(),
            &a.dividend_token_program.key(),
        );
        let vault = &a.flagship_dividend_vault;
        require_keys_eq!(vault.key(), expected, EndowmentError::WrongFlagshipVault);
        match read_token_account(vault)? {
            Some(view) if !is_frozen(vault)? => {
                require!(vault.is_writable, EndowmentError::WrongFlagshipVault);
                donation = donation.min(flagship_cap.saturating_sub(view.amount));
                if donation > 0 {
                    pay(vault.to_account_info(), donation)?;
                }
            }
            _ => donation = 0,
        }
    }

    // 6. Accounting.
    let liquidity_dividend = spent.saturating_sub(plan.swap_amount);
    let liquidity_coin = (coin_before + received).saturating_sub(coin_after);
    let lp_tokens = lp_after - lp_before;
    let config = &mut ctx.accounts.config;
    config.buy_allowance = plan.allowance.saturating_sub(spent);
    config.allowance_updated_at = now;
    config.last_buy_at = now;
    config.total_dividend_spent = config.total_dividend_spent.checked_add(spent).ok_or(EndowmentError::Overflow)?;
    config.total_coin_bought = config.total_coin_bought.checked_add(received).ok_or(EndowmentError::Overflow)?;
    config.total_coin_retained =
        config.total_coin_retained.checked_add(coin_after - coin_before).ok_or(EndowmentError::Overflow)?;
    config.total_liquidity_dividend =
        config.total_liquidity_dividend.checked_add(liquidity_dividend).ok_or(EndowmentError::Overflow)?;
    config.total_liquidity_coin =
        config.total_liquidity_coin.checked_add(liquidity_coin).ok_or(EndowmentError::Overflow)?;
    config.total_lp_tokens = config.total_lp_tokens.checked_add(lp_tokens).ok_or(EndowmentError::Overflow)?;
    config.total_tips = config.total_tips.checked_add(tip).ok_or(EndowmentError::Overflow)?;
    config.total_donated = config.total_donated.checked_add(donation).ok_or(EndowmentError::Overflow)?;
    config.record_completion(config_key, coin_after);

    emit!(Bought {
        config: config_key,
        dividend_spent: spent,
        coin_out: received,
        min_acceptable: floor,
        twap_price_x32: plan.twap_price_x32,
        liquidity_dividend,
        liquidity_coin,
        lp_tokens,
        tip,
        donation,
        total_dividend_spent: config.total_dividend_spent,
        total_coin_bought: config.total_coin_bought,
    });
    Ok(())
}

/// Every check and number the buy needs, before touching anything.
fn plan_buy(a: &Buyback, epoch: u64, now: i64) -> Result<Plan> {
    let config = &a.config;
    let params = &config.params;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(
        config.last_buy_at == 0 || now.saturating_sub(config.last_buy_at) >= params.min_buy_interval_secs,
        EndowmentError::BuyTooSoon
    );
    // Tradeable at all: hooks off, fees within their caps, the configured pool,
    // swaps enabled, nothing frozen. Sweeps run the same checks.
    let t = ensure_tradeable(
        config,
        &TradeAccounts {
            dividend_mint: &a.dividend_mint.to_account_info(),
            coin_mint: &a.coin_mint.to_account_info(),
            pool_state: &a.pool_state.to_account_info(),
            amm_config: &a.amm_config.to_account_info(),
            pool_dividend_vault: &a.pool_dividend_vault.to_account_info(),
            pool_coin_vault: &a.pool_coin_vault.to_account_info(),
            vaults: [&a.dividend_vault.to_account_info(), &a.coin_vault.to_account_info()],
        },
        epoch,
    )?;
    let (pool, dividend_index, coin_index) = (&t.pool, t.dividend_index, t.coin_index);
    let (pool_fee, dividend_fee, coin_fee) = (t.pool_fee, t.dividend_fee, t.coin_fee);
    require_keys_eq!(a.observation_state.key(), pool.observation, EndowmentError::WrongPool);
    require_keys_eq!(a.lp_mint.key(), pool.lp_mint, EndowmentError::WrongPool);
    let expected_lp_vault =
        get_associated_token_address_with_program_id(&a.authority.key(), &pool.lp_mint, &a.lp_token_program.key());
    require_keys_eq!(a.lp_vault.key(), expected_lp_vault, EndowmentError::WrongPool);
    let lp_vault_ready = read_token_account(&a.lp_vault)?
        .map(|v| v.mint == pool.lp_mint && v.owner == a.authority.key())
        .unwrap_or(false);

    // Price: the TWAP is the reference, from recorded history only. The coin's
    // spot price may not have risen above it by more than `max_twap_deviation_bps`
    // (plus the reader's own error bound). A coin that got *cheaper* than its
    // TWAP never refuses a buy: that is a good price, and the floor (measured
    // against the TWAP) bounds the fill either way (see `math`'s docs).
    let reserve_dividend = pool.reserve(dividend_index, a.pool_dividend_vault.amount)?;
    let reserve_coin = pool.reserve(coin_index, a.pool_coin_vault.amount)?;
    let spot = spot_price_x32(reserve_dividend, reserve_coin).ok_or(EndowmentError::InvalidPoolData)?;
    let twap = twap_price_x32(&a.observation_state.try_borrow_data()?, &a.pool_state.key(), dividend_index, now as u64)?;
    let twap_price_x32 = twap.price_x32;
    let tolerance = params.max_twap_deviation_bps as u128 + twap.error_bps as u128;
    // Fewer coin per dividend at spot than at the TWAP means the coin got pricier.
    require!(
        spot * 10_000 >= twap_price_x32 * 10_000u128.saturating_sub(tolerance),
        EndowmentError::PriceAboveTwap
    );

    // Size: what the vault can spend, capped per transaction, by the paced
    // allowance, and so this trade's own impact stays within half the budget.
    let allowance = refill(
        config.buy_allowance,
        config.allowance_updated_at,
        now,
        params.max_buy_per_day,
        params.max_buy_per_tx,
    );
    let extra_bps = params.tip_bps + config.donation_bps;
    let amount = spendable(a.dividend_vault.amount, extra_bps)
        .min(params.max_buy_per_tx)
        .min(allowance)
        .min(impact_cap(reserve_dividend, params.max_price_impact_bps));

    // After the milestone, split into buying and liquidity. While the pool has
    // deposits disabled or the LP vault doesn't exist yet, it all buys.
    let (to_buy, to_liquidity) = if config.milestone_reached && pool.deposits_enabled() && lp_vault_ready {
        split_buy(amount, params.buy_bps)
    } else {
        (amount, 0)
    };
    let lp_swap = if to_liquidity > 0 {
        zap_swap_amount(to_liquidity, reserve_dividend, pool_fee + dividend_fee + coin_fee)
    } else {
        0
    };
    let lp_deposit = to_liquidity - lp_swap;
    let swap_amount = to_buy + lp_swap;
    // Dust isn't worth a transaction; failing here leaves the buy interval unused.
    require!(
        swap_amount > 0 && swap_amount + lp_deposit >= params.min_buy_amount,
        EndowmentError::NothingToBuy
    );
    let quote = quote_out(swap_amount, reserve_dividend, reserve_coin, pool_fee, dividend_fee, coin_fee);

    Ok(Plan {
        dividend_index,
        coin_index,
        twap_price_x32,
        twap_error_bps: twap.error_bps,
        pool_fee,
        dividend_fee,
        coin_fee,
        allowance,
        swap_amount,
        quote,
        lp_swap,
        lp_deposit,
    })
}

/// CPI into Raydium CPMM `swap_base_input`, signed by the endowment's authority.
fn swap_base_input(ctx: &Context<Buyback>, config_key: &Pubkey, amount_in: u64, minimum_amount_out: u64) -> Result<()> {
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
            AccountMeta::new(a.dividend_vault.key(), false),
            AccountMeta::new(a.coin_vault.key(), false),
            AccountMeta::new(a.pool_dividend_vault.key(), false),
            AccountMeta::new(a.pool_coin_vault.key(), false),
            AccountMeta::new_readonly(a.dividend_token_program.key(), false),
            AccountMeta::new_readonly(a.coin_token_program.key(), false),
            AccountMeta::new_readonly(a.dividend_mint.key(), false),
            AccountMeta::new_readonly(a.coin_mint.key(), false),
            AccountMeta::new(a.observation_state.key(), false),
        ],
        data,
    };
    let bump = [a.config.authority_bump];
    let seeds = Config::authority_seeds(config_key, &bump);
    invoke_signed(
        &ix,
        &[
            a.authority.to_account_info(),
            a.cpmm_authority.to_account_info(),
            a.amm_config.to_account_info(),
            a.pool_state.to_account_info(),
            a.dividend_vault.to_account_info(),
            a.coin_vault.to_account_info(),
            a.pool_dividend_vault.to_account_info(),
            a.pool_coin_vault.to_account_info(),
            a.dividend_token_program.to_account_info(),
            a.coin_token_program.to_account_info(),
            a.dividend_mint.to_account_info(),
            a.coin_mint.to_account_info(),
            a.observation_state.to_account_info(),
            a.cpmm_program.to_account_info(),
        ],
        &[&seeds],
    )?;
    Ok(())
}

/// Whether the flagship coin can trade, so a donation to it can be spent: the
/// first remaining account must be its mint, and it must have no transfer hook
/// and no fee above the cap.
fn flagship_can_trade(remaining: &[AccountInfo], epoch: u64) -> Result<bool> {
    let mint = remaining.first().ok_or(EndowmentError::WrongFlagshipVault)?;
    require_keys_eq!(mint.key(), FLAGSHIP_COIN_MINT, EndowmentError::WrongFlagshipVault);
    if mint.data_is_empty() || hook_enabled(mint)? {
        return Ok(false);
    }
    Ok(capped_transfer_fee_bps(mint, epoch).is_ok())
}

/// The flagship's dividend-vault cap, if a donation can go to it now: its coin
/// can trade (`flagship_can_trade`, first remaining account), and its config
/// (second remaining account, at `flagship_config()`) exists, isn't retired and
/// isn't paused. `None` skips the donation.
fn flagship_state(remaining: &[AccountInfo], epoch: u64, program_id: &Pubkey) -> Result<Option<u64>> {
    if !flagship_can_trade(remaining, epoch)? {
        return Ok(None);
    }
    let info = remaining.get(1).ok_or(EndowmentError::WrongFlagshipVault)?;
    require_keys_eq!(info.key(), flagship_config(), EndowmentError::WrongFlagshipVault);
    if info.data_is_empty() || info.owner != program_id {
        return Ok(None);
    }
    let data = info.try_borrow_data()?;
    let config = Config::try_deserialize(&mut &data[..])?;
    let now = Clock::get()?.unix_timestamp;
    if config.retired || config.is_paused(now) {
        return Ok(None);
    }
    Ok(Some(config.vault_cap()))
}

/// Deposits the liquidity share into the pool at its current ratio, with the LP
/// tokens landing in the authority's LP account. Skipped (the share stays in the
/// vault for a later buy) if the pool's price after our swap has strayed from
/// the TWAP by more than the impact budget plus the allowed drift, or if the
/// amounts are too small to mint any LP.
fn deposit_liquidity(ctx: &Context<Buyback>, config_key: &Pubkey, plan: &Plan, coin_budget: u64) -> Result<()> {
    let a = &ctx.accounts;

    // Reserves after our own swap, as Raydium will see them.
    let pool = PoolView::parse(&a.pool_state.try_borrow_data()?)?;
    let reserve_dividend = pool.reserve(plan.dividend_index, token_amount(&a.pool_dividend_vault.to_account_info())?)?;
    let reserve_coin = pool.reserve(plan.coin_index, token_amount(&a.pool_coin_vault.to_account_info())?)?;
    let spot = spot_price_x32(reserve_dividend, reserve_coin).ok_or(EndowmentError::InvalidPoolData)?;
    // Symmetric, unlike the buy band: skipping a deposit is harmless (the share
    // stays in the vault for a later buy), while depositing at a ratio pushed
    // away from the TWAP in either direction locks liquidity in at a price that
    // then reverts. Includes the reader's error bound like the buy band.
    let band = a.config.params.max_price_impact_bps as u128
        + a.config.params.max_twap_deviation_bps as u128
        + plan.twap_error_bps as u128;
    let twap = plan.twap_price_x32;
    if spot * 10_000 < twap * 10_000u128.saturating_sub(band) || spot * 10_000 > twap * (10_000 + band) {
        return Ok(());
    }

    // Both sides may pay a transfer fee on the way in; budget for what arrives.
    let net = |amount: u64, fee_bps: u64| (amount as u128 * (10_000 - fee_bps as u128) / 10_000) as u64;
    let dividend_net = net(plan.lp_deposit, plan.dividend_fee).saturating_sub(1);
    let coin_net = net(coin_budget, plan.coin_fee).saturating_sub(1);
    let lp_amount = lp_tokens_for(dividend_net, coin_net, reserve_dividend, reserve_coin, pool.lp_supply);
    if lp_amount == 0 {
        return Ok(());
    }

    let mut max = [0u64; 2];
    max[plan.dividend_index] = plan.lp_deposit;
    max[plan.coin_index] = coin_budget;
    let mut data = DEPOSIT_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&lp_amount.to_le_bytes());
    data.extend_from_slice(&max[0].to_le_bytes());
    data.extend_from_slice(&max[1].to_le_bytes());

    let our_account = |i: usize| {
        if i == plan.dividend_index { a.dividend_vault.to_account_info() } else { a.coin_vault.to_account_info() }
    };
    let pool_vault = |i: usize| {
        if i == plan.dividend_index { a.pool_dividend_vault.to_account_info() } else { a.pool_coin_vault.to_account_info() }
    };
    let mint = |i: usize| {
        if i == plan.dividend_index { a.dividend_mint.to_account_info() } else { a.coin_mint.to_account_info() }
    };

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
        a.token_2022_program.to_account_info(),
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
    let bump = [a.config.authority_bump];
    let seeds = Config::authority_seeds(config_key, &bump);
    invoke_signed(&ix, &all, &[&seeds])?;
    Ok(())
}

/// Balance of an SPL token account (legacy or Token-2022), from its raw data.
fn token_amount(info: &AccountInfo) -> Result<u64> {
    let data = info.try_borrow_data()?;
    require!(data.len() >= 72, EndowmentError::WrongPool);
    Ok(u64::from_le_bytes(data[64..72].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// The $PENIS mint as fetched from mainnet into the integration fixtures.
    fn flagship_mint_data() -> Vec<u8> {
        let path = format!("{}/tests/fixtures/token_1_mint.json", env!("CARGO_MANIFEST_DIR"));
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        base64::engine::general_purpose::STANDARD.decode(json["data"].as_str().unwrap()).unwrap()
    }

    /// $PENIS TransferFeeConfig: the newer fee's epoch and basis points.
    const NEWER_FEE_EPOCH: usize = 328;
    const NEWER_FEE_BPS: usize = 344;

    fn can_trade(key: Pubkey, mut data: Vec<u8>, epoch: u64) -> Result<bool> {
        let owner = anchor_spl::token_2022::ID;
        let mut lamports = 1_000_000;
        let info = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);
        flagship_can_trade(&[info], epoch)
    }

    #[test]
    fn r3mint03_the_donation_is_skipped_while_the_flagship_coin_cant_trade() {
        let data = flagship_mint_data();
        assert!(can_trade(FLAGSHIP_COIN_MINT, data.clone(), 0).unwrap());
        // A fee raised above the cap (scheduled for any epoch): skipped.
        let mut raised = data.clone();
        raised[NEWER_FEE_BPS..NEWER_FEE_BPS + 2].copy_from_slice(&501u16.to_le_bytes());
        raised[NEWER_FEE_EPOCH..NEWER_FEE_EPOCH + 8].copy_from_slice(&1_000u64.to_le_bytes());
        assert!(!can_trade(FLAGSHIP_COIN_MINT, raised, 0).unwrap());
        // Only the flagship's own mint will do, and it must be passed.
        assert!(can_trade(Pubkey::new_unique(), data, 0).is_err());
        assert!(flagship_can_trade(&[], 0).is_err());
    }
}
