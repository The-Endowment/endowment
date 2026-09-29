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
    token_2022::{
        spl_token_2022::{
            extension::{transfer_fee::TransferFeeConfig, BaseStateWithExtensions, StateWithExtensions},
            state::Mint as MintState,
        },
        Token2022,
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

/// Permissionless: anyone may crank a buyback for an endowment, and is paid a
/// small tip in the dividend asset for it. The contract decides the size. The
/// dividend can only leave the dividend vault through the endowment's own
/// Raydium pool, as the capped tip, or as the donation locked in at creation;
/// the coin can only land in the endowment's coin vault. After contributions
/// close, part of each buyback becomes liquidity whose LP tokens land in an
/// authority-owned account that nothing can withdraw from.
///
/// Remaining accounts: dividend transfer-hook extras for the tip and donation,
/// only if the dividend mint's hook is ever switched on (see `transfer.rs`).
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
    /// CHECK: the flagship endowment's dividend vault. Only touched, and then
    /// checked against its derived address, when this endowment donates.
    #[account(mut)]
    pub flagship_dividend_vault: UncheckedAccount<'info>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub coin_token_program: Interface<'info, TokenInterface>,
    /// Raydium LP mints use the original SPL Token program.
    pub lp_token_program: Program<'info, Token>,
    /// Raydium's deposit takes both token programs explicitly.
    pub token_2022_program: Program<'info, Token2022>,
}

pub fn handle_buyback<'info>(ctx: Context<'info, Buyback<'info>>, min_out: u64) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let accounts = &ctx.accounts;
    let config = &accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(
        config.last_buy_at == 0 || now.saturating_sub(config.last_buy_at) >= config.min_buy_interval_secs,
        EndowmentError::BuyTooSoon
    );

    // The contract sizes the buy, leaving room for the tip and donation.
    let (day_start, bought_today) = roll_day(config.day_start, config.bought_today, now);
    let left_today = config.max_buy_per_day.saturating_sub(bought_today);
    let extra_bps = config.tip_bps + config.donation_bps;
    let amount_in = buy_amount(accounts.dividend_vault.amount, extra_bps, config.max_buy_per_tx, left_today);
    require!(amount_in > 0, EndowmentError::NothingToBuy);
    let bought_today = bought_today.checked_add(amount_in).ok_or(EndowmentError::Overflow)?;

    // Every Raydium account must belong to the configured pool, in whichever
    // order the pool holds the two mints.
    let pool = PoolView::parse(&accounts.pool_state.try_borrow_data()?)?;
    let dividend_index = pool.index_of(&config.dividend_mint)?;
    let coin_index = pool.index_of(&config.coin_mint)?;
    require_keys_eq!(accounts.amm_config.key(), pool.amm_config, EndowmentError::WrongPool);
    require_keys_eq!(accounts.observation_state.key(), pool.observation, EndowmentError::WrongPool);
    require_keys_eq!(accounts.pool_dividend_vault.key(), pool.vaults[dividend_index], EndowmentError::WrongPool);
    require_keys_eq!(accounts.pool_coin_vault.key(), pool.vaults[coin_index], EndowmentError::WrongPool);

    // Contributions close as soon as the vault reaches the cap.
    let mut closing_now = false;
    let closed = config.closed || {
        closing_now = accounts.coin_vault.amount >= config.contribution_cap;
        closing_now
    };
    let (swap_amount, deposit_dividend, lp_swap) =
        if closed { split_buy(amount_in, config.buy_bps) } else { (amount_in, 0, 0) };

    let transfer_fee = transfer_fee_bps(&accounts.coin_mint.to_account_info(), clock.epoch)?;
    let coin_before = accounts.coin_vault.amount;
    let dividend_before = accounts.dividend_vault.amount;

    // 1. Swap the dividend for the coin, protected by the price floor.
    let mut received = 0;
    let mut floor = 0;
    if swap_amount > 0 {
        let reserve_in = accounts.pool_dividend_vault.amount.saturating_sub(pool.reserved_fees[dividend_index]);
        let reserve_out = accounts.pool_coin_vault.amount.saturating_sub(pool.reserved_fees[coin_index]);
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

        swap_base_input(&ctx, &config_key, swap_amount, floor.max(min_out))?;
        ctx.accounts.coin_vault.reload()?;
        received = ctx.accounts.coin_vault.amount.saturating_sub(coin_before);
        require!(received >= floor, EndowmentError::PriceImpactTooHigh);
        require!(received >= min_out, EndowmentError::SlippageExceeded);
    }

    // 2. After close: deposit the liquidity share into the pool, locking the LP tokens.
    let mut lp_tokens = 0;
    if deposit_dividend > 0 && received > 0 {
        let coin_budget = (received as u128 * lp_swap as u128 / swap_amount as u128) as u64;
        lp_tokens = deposit_liquidity(
            &ctx,
            &config_key,
            &pool,
            dividend_index,
            coin_index,
            deposit_dividend,
            coin_budget,
            transfer_fee,
        )?;
    }

    // The endowment's coin never shrinks.
    ctx.accounts.coin_vault.reload()?;
    let coin_after = ctx.accounts.coin_vault.amount;
    require!(coin_after >= coin_before, EndowmentError::VaultWouldShrink);

    let bump = [ctx.accounts.config.authority_bump];
    let seeds = Config::authority_seeds(&config_key, &bump);

    // 3. Pay the caller's tip.
    let tip = (amount_in as u128 * ctx.accounts.config.tip_bps as u128 / 10_000) as u64;
    if tip > 0 {
        transfer_checked_with_hook(
            &ctx.accounts.dividend_token_program.to_account_info(),
            &ctx.accounts.dividend_vault.to_account_info(),
            &ctx.accounts.dividend_mint.to_account_info(),
            &ctx.accounts.caller_dividend_account.to_account_info(),
            &ctx.accounts.authority.to_account_info(),
            ctx.remaining_accounts,
            tip,
            ctx.accounts.dividend_mint.decimals,
            &[&seeds],
        )?;
    }

    // 4. Send the donation, if this endowment chose one, to the flagship's dividend vault.
    let donation = (amount_in as u128 * ctx.accounts.config.donation_bps as u128 / 10_000) as u64;
    if donation > 0 {
        let (flagship_authority, _) =
            Pubkey::find_program_address(&[AUTHORITY_SEED, FLAGSHIP_CONFIG.as_ref()], ctx.program_id);
        let expected = get_associated_token_address_with_program_id(
            &flagship_authority,
            &ctx.accounts.dividend_mint.key(),
            &ctx.accounts.dividend_token_program.key(),
        );
        require_keys_eq!(
            ctx.accounts.flagship_dividend_vault.key(),
            expected,
            EndowmentError::WrongFlagshipVault
        );
        transfer_checked_with_hook(
            &ctx.accounts.dividend_token_program.to_account_info(),
            &ctx.accounts.dividend_vault.to_account_info(),
            &ctx.accounts.dividend_mint.to_account_info(),
            &ctx.accounts.flagship_dividend_vault.to_account_info(),
            &ctx.accounts.authority.to_account_info(),
            ctx.remaining_accounts,
            donation,
            ctx.accounts.dividend_mint.decimals,
            &[&seeds],
        )?;
    }

    ctx.accounts.dividend_vault.reload()?;
    let dividend_spent = dividend_before
        .saturating_sub(ctx.accounts.dividend_vault.amount)
        .saturating_sub(tip)
        .saturating_sub(donation);
    let liquidity_dividend = dividend_spent.saturating_sub(swap_amount);

    let config = &mut ctx.accounts.config;
    if closing_now && !config.closed {
        config.closed = true;
        emit!(ContributionsClosed { config: config_key, by_cap: true, coin_held: coin_before });
    }
    if !config.closed && coin_after >= config.contribution_cap {
        config.closed = true;
        emit!(ContributionsClosed { config: config_key, by_cap: true, coin_held: coin_after });
    }
    config.day_start = day_start;
    config.bought_today = bought_today;
    config.last_buy_at = now;
    config.total_dividend_spent =
        config.total_dividend_spent.checked_add(dividend_spent).ok_or(EndowmentError::Overflow)?;
    config.total_coin_bought = config.total_coin_bought.checked_add(received).ok_or(EndowmentError::Overflow)?;
    config.total_tips = config.total_tips.checked_add(tip).ok_or(EndowmentError::Overflow)?;
    config.total_donated = config.total_donated.checked_add(donation).ok_or(EndowmentError::Overflow)?;
    config.total_liquidity_dividend = config
        .total_liquidity_dividend
        .checked_add(liquidity_dividend)
        .ok_or(EndowmentError::Overflow)?;
    config.total_lp_tokens = config.total_lp_tokens.checked_add(lp_tokens).ok_or(EndowmentError::Overflow)?;

    emit!(Bought {
        config: config_key,
        dividend_in: amount_in,
        coin_out: received,
        min_acceptable: floor,
        liquidity_dividend,
        lp_tokens,
        tip,
        donation,
        total_dividend_spent: config.total_dividend_spent,
        total_coin_bought: config.total_coin_bought,
    });
    Ok(())
}

/// A Token-2022 mint's transfer fee for this epoch, in basis points (0 if none,
/// including for original SPL Token mints).
fn transfer_fee_bps(mint: &AccountInfo, epoch: u64) -> Result<u64> {
    let data = mint.try_borrow_data()?;
    let state = StateWithExtensions::<MintState>::unpack(&data)?;
    Ok(match state.get_extension::<TransferFeeConfig>() {
        Ok(config) => u16::from(config.get_epoch_fee(epoch).transfer_fee_basis_points) as u64,
        Err(_) => 0,
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

/// Deposits up to `dividend` and `coin_budget` into the pool at its current
/// ratio, with the LP tokens landing in the authority's LP account. Returns the
/// LP tokens minted (0 if the amounts are too small to mint any).
#[allow(clippy::too_many_arguments)]
fn deposit_liquidity(
    ctx: &Context<Buyback>,
    config_key: &Pubkey,
    pool_before_swap: &PoolView,
    dividend_index: usize,
    coin_index: usize,
    dividend: u64,
    coin_budget: u64,
    transfer_fee_bps: u64,
) -> Result<u64> {
    let a = &ctx.accounts;
    require_keys_eq!(a.lp_mint.key(), pool_before_swap.lp_mint, EndowmentError::WrongPool);
    let expected_lp_vault =
        get_associated_token_address_with_program_id(&a.authority.key(), &a.lp_mint.key(), &a.lp_token_program.key());
    require_keys_eq!(a.lp_vault.key(), expected_lp_vault, EndowmentError::WrongPool);

    // Reserves after our own swap, as Raydium will see them.
    let pool = PoolView::parse(&a.pool_state.try_borrow_data()?)?;
    let reserve_dividend =
        token_amount(&a.pool_dividend_vault.to_account_info())?.saturating_sub(pool.reserved_fees[dividend_index]);
    let reserve_coin =
        token_amount(&a.pool_coin_vault.to_account_info())?.saturating_sub(pool.reserved_fees[coin_index]);

    // The coin may pay a transfer fee on the way in; budget for what arrives.
    let coin_net = (coin_budget as u128 * (10_000 - transfer_fee_bps as u128) / 10_000) as u64;
    let lp_amount = lp_tokens_for(dividend, coin_net.saturating_sub(1), reserve_dividend, reserve_coin, pool.lp_supply);
    if lp_amount == 0 {
        return Ok(0);
    }

    let lp_before = token_amount(&a.lp_vault.to_account_info())?;
    let mut max = [0u64; 2];
    max[dividend_index] = dividend;
    max[coin_index] = coin_budget;
    let mut data = DEPOSIT_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&lp_amount.to_le_bytes());
    data.extend_from_slice(&max[0].to_le_bytes());
    data.extend_from_slice(&max[1].to_le_bytes());

    let our_account =
        |i: usize| if i == dividend_index { a.dividend_vault.to_account_info() } else { a.coin_vault.to_account_info() };
    let pool_vault = |i: usize| {
        if i == dividend_index { a.pool_dividend_vault.to_account_info() } else { a.pool_coin_vault.to_account_info() }
    };
    let mint =
        |i: usize| if i == dividend_index { a.dividend_mint.to_account_info() } else { a.coin_mint.to_account_info() };

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

    let lp_after = token_amount(&a.lp_vault.to_account_info())?;
    Ok(lp_after.saturating_sub(lp_before))
}

/// Balance of an SPL token account (legacy or Token-2022), from its raw data.
fn token_amount(info: &AccountInfo) -> Result<u64> {
    let data = info.try_borrow_data()?;
    require!(data.len() >= 72, EndowmentError::WrongPool);
    Ok(u64::from_le_bytes(data[64..72].try_into().unwrap()))
}
