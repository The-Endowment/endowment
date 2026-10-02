use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::get_associated_token_address_with_program_id,
    token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked},
};

use crate::{
    constants::*,
    error::EndowmentError,
    events::Swept,
    health::{ensure_tradeable, TradeAccounts},
    raydium::CPMM_PROGRAM_ID,
    state::{Config, Landlord},
};

/// Permissionless: anyone may crank a sweep. Funds can only move from the
/// landlord's delegated dividend account into this endowment's dividend vault,
/// and only the part above the landlord's baseline. A sweep never changes the
/// baseline: a dip below it sweeps nothing and leaves it where it is. With the
/// reward allowance on, a sweep also never takes more than the landlord's
/// allowance: what its counted coin earned (see `post_reward_total`).
///
/// Sweeps stop for good once the coin vault holds the goal (`contribution_cap`,
/// bought or sent directly; recorded here as well as by buybacks) or the admin
/// retires the endowment. They run only while the endowment is active and a
/// count has finished recently (`Config::sweeps_on`), and they fail closed
/// whenever a buyback couldn't run (`health::ensure_tradeable`): a transfer hook
/// switched on, either mint's transfer fee above the cap, the pool's swaps
/// disabled or its fee above the cap, or a vault frozen. Dividends then stay
/// with landlords.
#[derive(Accounts)]
pub struct Sweep<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: this endowment's authority PDA, which signs as the delegate.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        mut,
        seeds = [LANDLORD_SEED, config.key().as_ref(), landlord.owner.as_ref()],
        bump = landlord.bump,
        has_one = config,
    )]
    pub landlord: Box<Account<'info, Landlord>>,

    #[account(address = config.dividend_mint)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        address = landlord.dividend_account,
        constraint = dividend_account.owner == landlord.owner @ EndowmentError::NotDelegated,
    )]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = dividend_mint,
        associated_token::authority = authority,
        associated_token::token_program = dividend_token_program,
    )]
    pub dividend_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// What a buyback would trade through; read only, to check it can.
    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    /// The endowment's coin vault, at its derived address: its balance decides
    /// whether the goal has been reached, and its frozen state is checked.
    #[account(
        address = get_associated_token_address_with_program_id(
            &authority.key(),
            &config.coin_mint,
            coin_mint.to_account_info().owner,
        ) @ EndowmentError::WrongPool,
        constraint = coin_vault.mint == config.coin_mint && coin_vault.owner == authority.key()
            @ EndowmentError::WrongPool,
    )]
    pub coin_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the endowment's pool; parsed by `ensure_tradeable`.
    #[account(address = config.pool @ EndowmentError::WrongPool, owner = CPMM_PROGRAM_ID)]
    pub pool_state: UncheckedAccount<'info>,
    /// CHECK: the pool's AMM config; checked against the pool.
    #[account(owner = CPMM_PROGRAM_ID)]
    pub amm_config: UncheckedAccount<'info>,
    /// CHECK: the pool's dividend vault; checked against the pool.
    pub pool_dividend_vault: UncheckedAccount<'info>,
    /// CHECK: the pool's coin vault; checked against the pool.
    pub pool_coin_vault: UncheckedAccount<'info>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
}

pub fn handle_sweep(ctx: Context<Sweep>) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    // The goal ends contributions for good. Record it (even while paused) and
    // stop without failing, so the record is kept and nothing moves.
    let coin_held = ctx.accounts.coin_vault.amount;
    if ctx.accounts.config.record_completion(config_key, coin_held) {
        return Ok(());
    }
    let config = &ctx.accounts.config;
    require!(!config.is_paused(now), EndowmentError::Paused);
    require!(!config.retired, EndowmentError::Retired);
    require!(config.active, EndowmentError::NotActive);
    require!(config.sweeps_on(now), EndowmentError::CountStale);
    let a = &ctx.accounts;
    ensure_tradeable(
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
        clock.epoch,
    )?;

    let dividend_account = &ctx.accounts.dividend_account;
    require!(
        dividend_account.delegate == Some(ctx.accounts.authority.key()).into(),
        EndowmentError::NotDelegated
    );
    // Never more than the vault can spend in MAX_VAULT_DAYS_OF_BUYS days: the
    // rest stays with the landlord for a later sweep (R3-MINT-01, FC-R3-04).
    let vault_before = ctx.accounts.dividend_vault.amount;
    let vault_cap = config.vault_cap();
    let (balance, delegated) = (dividend_account.amount, dividend_account.delegated_amount);
    let (reward_index, carry_floor, capped, authority_bump) =
        (config.reward_index, config.carry_floor(), config.params.allowance_margin_bps > 0, config.authority_bump);
    let landlord = &mut ctx.accounts.landlord;
    // And, with the allowance on, never more than what its coin earned.
    landlord.settle(reward_index, carry_floor);
    let mut amount = landlord.sweepable(balance, delegated).min(vault_cap.saturating_sub(vault_before));
    if capped {
        amount = amount.min(landlord.allowance);
    }
    if amount == 0 {
        return Ok(());
    }

    let bump = [authority_bump];
    let seeds = Config::authority_seeds(&config_key, &bump);
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.dividend_token_program.key(),
            TransferChecked {
                from: ctx.accounts.dividend_account.to_account_info(),
                mint: ctx.accounts.dividend_mint.to_account_info(),
                to: ctx.accounts.dividend_vault.to_account_info(),
                authority: ctx.accounts.authority.to_account_info(),
            },
            &[&seeds],
        ),
        amount,
        ctx.accounts.dividend_mint.decimals,
    )?;
    // What actually arrived, net of any transfer fee on the dividend.
    ctx.accounts.dividend_vault.reload()?;
    let received = ctx.accounts.dividend_vault.amount.saturating_sub(vault_before);

    let landlord = &mut ctx.accounts.landlord;
    landlord.total_contributed = landlord.total_contributed.checked_add(amount).ok_or(EndowmentError::Overflow)?;
    landlord.last_sweep_at = now;
    if capped {
        landlord.allowance -= amount;
    }
    let config = &mut ctx.accounts.config;
    config.total_swept = config.total_swept.checked_add(received).ok_or(EndowmentError::Overflow)?;
    config.last_sweep_at = now;

    emit!(Swept {
        config: config_key,
        owner: landlord.owner,
        amount,
        received,
        baseline: landlord.baseline,
        total_contributed: landlord.total_contributed,
        allowance: landlord.allowance,
    });
    Ok(())
}
