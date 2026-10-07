use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::get_associated_token_address_with_program_id,
    token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked},
};

use crate::{
    collection::*,
    constants::*,
    error::EndowmentError,
    events::Swept,
    health::{ensure_tradeable, TradeAccounts},
    raydium::CPMM_PROGRAM_ID,
    state::{Config, Landlord},
};

/// Collector-signed, exact-amount collection into separate refundable custody.
/// Report evidence is an attestation, not an on-chain proof of reward origin.
/// The legacy no-argument ABI fails closed. Buybacks cannot sign for this vault.
#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct Sweep<'info> {
    #[account(mut, address = policy.collector @ EndowmentError::NotCollector)]
    pub collector: Signer<'info>,
    #[account(mut, seeds = [POLICY_SEED, config.key().as_ref()], bump = policy.bump, has_one = config)]
    pub policy: Box<Account<'info, CollectionPolicy>>,
    #[account(mut, seeds = [CONSENT_SEED, config.key().as_ref(), landlord.owner.as_ref()],
        bump = consent.bump, has_one = config,
        constraint = consent.owner == landlord.owner @ EndowmentError::InvalidCollection)]
    pub consent: Box<Account<'info, CollectionConsent>>,
    #[account(init, payer = collector, space = 8 + PendingCollection::INIT_SPACE,
        seeds = [RECEIPT_SEED, config.key().as_ref(), landlord.owner.as_ref(), &nonce.to_le_bytes()], bump)]
    pub receipt: Box<Account<'info, PendingCollection>>,
    #[account(mut)] // Identity validated by validate_pending_vault before any transfer.
    pub pending_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub system_program: Program<'info, System>,
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

pub fn handle_sweep(ctx: Context<Sweep>, nonce: u64, report: CollectionReport) -> Result<()> {
    validate_pending_vault(&ctx.accounts)?;
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    // The goal ends contributions for good. Record it (even while paused) and
    // stop without failing, so the record is kept and nothing moves.
    let coin_held = ctx.accounts.coin_vault.amount;
    if ctx.accounts.config.record_completion(config_key, coin_held) {
        ctx.accounts.receipt.close(ctx.accounts.collector.to_account_info())?;
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

    crate::transfer::ensure_refundable_dividend(&ctx.accounts.dividend_mint.to_account_info())?;
    let consent = &ctx.accounts.consent;
    require!(consent.enabled, EndowmentError::CollectionConsentRequired);
    require!(
        report.consent_epoch == consent.epoch
            && nonce == consent.next_nonce
            && report.valid_until >= now
            && report.valid_until <= now.saturating_add(REPORT_SECONDS)
            && report.evidence_hash != [0; 32],
        EndowmentError::InvalidCollection
    );
    let dividend_account = &ctx.accounts.dividend_account;
    require!(
        report.expected_balance == dividend_account.amount,
        EndowmentError::InvalidCollection
    );
    require!(
        dividend_account.delegate == Some(ctx.accounts.authority.key()).into(),
        EndowmentError::NotDelegated
    );
    // Never more than the vault can spend in MAX_VAULT_DAYS_OF_BUYS days: the
    // rest stays with the landlord for a later sweep (R3-MINT-01, FC-R3-04).
    let vault_before = ctx.accounts.pending_vault.amount;
    require!(
        vault_before >= ctx.accounts.policy.pending,
        EndowmentError::InvalidCollection
    );
    let committed = ctx
        .accounts
        .dividend_vault
        .amount
        .checked_add(ctx.accounts.policy.pending)
        .ok_or(EndowmentError::Overflow)?;
    let vault_cap = config.vault_cap();
    let (balance, delegated) = (dividend_account.amount, dividend_account.delegated_amount);
    let authority_bump = config.authority_bump;
    let landlord = &mut ctx.accounts.landlord;
    // An omitted or invalidated balance cannot authorize a collection.
    config.settle_landlord(landlord, now);
    let amount = landlord
        .sweepable(balance, delegated)
        .min(vault_cap.saturating_sub(committed))
        .min(report.amount)
        .min(landlord.allowance);
    if amount == 0 {
        // An accepted report is consumed even if another collection filled the
        // vault first. Durable workers can distinguish this finalized no-op
        // from a still-pending send; replaying it cannot collect a later reward.
        ctx.accounts.consent.next_nonce = ctx
            .accounts
            .consent
            .next_nonce
            .checked_add(1)
            .ok_or(EndowmentError::Overflow)?;
        ctx.accounts.receipt.close(ctx.accounts.collector.to_account_info())?;
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
                to: ctx.accounts.pending_vault.to_account_info(),
                authority: ctx.accounts.authority.to_account_info(),
            },
            &[&seeds],
        ),
        amount,
        ctx.accounts.dividend_mint.decimals,
    )?;
    // What actually arrived, net of any transfer fee on the dividend.
    ctx.accounts.pending_vault.reload()?;
    let received = ctx.accounts.pending_vault.amount.saturating_sub(vault_before);
    require!(received == amount, EndowmentError::UnsupportedRefundMint);
    let consent = &mut ctx.accounts.consent;
    consent.next_nonce = consent.next_nonce.checked_add(1).ok_or(EndowmentError::Overflow)?;
    let release_at = now.checked_add(HOLD_SECONDS).ok_or(EndowmentError::Overflow)?;
    let refund_at = now.checked_add(REFUND_SECONDS).ok_or(EndowmentError::Overflow)?;
    ctx.accounts.receipt.set_inner(PendingCollection {
        config: config_key,
        owner: ctx.accounts.landlord.owner,
        payer: ctx.accounts.collector.key(),
        nonce: nonce,
        consent_epoch: consent.epoch,
        amount: received,
        collected_at: now,
        release_at,
        refund_at,
        collection_evidence: report.evidence_hash,
        reviewed: false,
        approved_amount: 0,
        review_evidence: [0; 32],
        bump: ctx.bumps.receipt,
    });
    ctx.accounts.policy.pending = ctx
        .accounts
        .policy
        .pending
        .checked_add(received)
        .ok_or(EndowmentError::Overflow)?;
    emit!(CollectionHeld {
        config: config_key,
        owner: ctx.accounts.landlord.owner,
        nonce: nonce,
        amount: received,
        release_at,
        refund_at,
        evidence_hash: report.evidence_hash
    });

    let landlord = &mut ctx.accounts.landlord;
    landlord.total_contributed = landlord
        .total_contributed
        .checked_add(amount)
        .ok_or(EndowmentError::Overflow)?;
    landlord.last_sweep_at = now;
    landlord.allowance -= amount;
    let config = &mut ctx.accounts.config;
    config.total_swept = config
        .total_swept
        .checked_add(received)
        .ok_or(EndowmentError::Overflow)?;
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

#[inline(never)]
fn validate_pending_vault(a: &Sweep) -> Result<()> {
    require_keys_eq!(
        a.pending_vault.key(),
        get_associated_token_address_with_program_id(
            &a.policy.key(),
            &a.config.dividend_mint,
            &a.dividend_token_program.key()
        ),
        EndowmentError::InvalidCollection
    );
    require!(
        a.pending_vault.owner == a.policy.key() && a.pending_vault.mint == a.config.dividend_mint,
        EndowmentError::InvalidCollection
    );
    Ok(())
}
