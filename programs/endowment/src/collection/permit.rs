//! Independent pre-collection approval for holders who explicitly choose
//! transaction-history accounting. A permit is an attestation, not an on-chain
//! proof of payout origin. Both roles must reconcile history independently.
use super::state::*;
use crate::{
    constants::*,
    error::EndowmentError,
    state::{Config, Landlord},
};
use anchor_lang::prelude::*;
use anchor_spl::token_interface::TokenAccount;

pub const PERMIT_SEED: &[u8] = b"collection_permit";
// Covers a five-minute worker interval plus a slow pass; the collector still
// uses a <=60-second report and independently checks history before signing.
pub const PERMIT_SECONDS: i64 = 600;

/// Reused per wallet; the durable consent nonce makes each authorization
/// single-use, including zero-amount collections. No funds are held here.
#[account]
#[derive(InitSpace)]
pub struct CollectionPermit {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub collector: Pubkey,
    pub reviewer: Pubkey,
    pub consent_epoch: u64,
    pub nonce: u64,
    pub expected_balance: u64,
    pub amount: u64,
    pub valid_until: i64,
    pub count_at: i64,
    pub refresher_epoch: u32,
    pub pause_started_at: i64,
    pub evidence_hash: [u8; 32],
    pub bump: u8,
}

#[derive(Accounts)]
pub struct AuthorizeCollection<'info> {
    #[account(mut, address = policy.reviewer @ EndowmentError::NotReviewer)]
    pub reviewer: Signer<'info>,
    /// CHECK: identity is bound by the consent and landlord accounts below.
    pub owner: UncheckedAccount<'info>,
    #[account(seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [POLICY_SEED, config.key().as_ref()], bump = policy.bump, has_one = config)]
    pub policy: Box<Account<'info, CollectionPolicy>>,
    #[account(seeds = [CONSENT_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump = consent.bump, has_one = config, has_one = owner)]
    pub consent: Box<Account<'info, CollectionConsent>>,
    #[account(mut, seeds = [LANDLORD_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump = landlord.bump, has_one = config, has_one = owner)]
    pub landlord: Box<Account<'info, Landlord>>,
    #[account(address = landlord.dividend_account,
        constraint = dividend_account.owner == owner.key() && dividend_account.mint == config.dividend_mint
            @ EndowmentError::NotDelegated)]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the only allowed PUMP delegate, derived for this config.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(init_if_needed, payer = reviewer, space = 8 + CollectionPermit::INIT_SPACE,
        seeds = [PERMIT_SEED, config.key().as_ref(), owner.key().as_ref()], bump)]
    pub permit: Box<Account<'info, CollectionPermit>>,
    pub system_program: Program<'info, System>,
}

pub fn authorize(
    ctx: Context<AuthorizeCollection>,
    nonce: u64,
    report: CollectionReport,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let a = ctx.accounts;
    require!(
        a.landlord.tracked_rewards(),
        EndowmentError::InvalidCollection
    );
    require!(a.consent.enabled, EndowmentError::CollectionConsentRequired);
    require!(!a.config.is_paused(now), EndowmentError::Paused);
    require!(
        !a.config.retired && !a.config.milestone_reached,
        EndowmentError::Completed
    );
    require!(a.config.sweeps_on(now), EndowmentError::CountStale);
    require!(a.config.active, EndowmentError::NotActive);
    require!(
        CollectionPolicy::valid_roles(&a.policy.collector, &a.policy.reviewer),
        EndowmentError::InvalidCollection
    );
    require!(
        a.dividend_account.delegate == Some(a.authority.key()).into(),
        EndowmentError::NotDelegated
    );
    require!(
        nonce == a.consent.next_nonce
            && report.consent_epoch == a.consent.epoch
            && report.expected_balance == a.dividend_account.amount
            && report.amount > 0
            && report.valid_until > now
            && report.valid_until <= now.saturating_add(PERMIT_SECONDS)
            && report.evidence_hash != [0; 32],
        EndowmentError::InvalidCollection
    );
    a.config.settle_landlord(&mut a.landlord, now);
    require!(
        report.amount
            <= a.dividend_account
                .amount
                .min(a.dividend_account.delegated_amount)
                .min(a.landlord.allowance),
        EndowmentError::InvalidCollection
    );
    a.permit.set_inner(CollectionPermit {
        config: a.config.key(),
        owner: a.owner.key(),
        collector: a.policy.collector,
        reviewer: a.policy.reviewer,
        consent_epoch: a.consent.epoch,
        nonce,
        expected_balance: report.expected_balance,
        amount: report.amount,
        valid_until: report.valid_until,
        count_at: a.config.last_count_at,
        refresher_epoch: a.config.refresher_epoch,
        pause_started_at: a.config.pause_started_at,
        evidence_hash: report.evidence_hash,
        bump: ctx.bumps.permit,
    });
    Ok(())
}

/// Used only by tracked collection. Legacy collections keep the fixed floor.
/// Account identity, roles, consent, nonce, time and balance must all match;
/// neither a collector-only report nor an old permit can move the floor.
pub fn approved_amount(
    info: Option<&AccountInfo>,
    config_key: &Pubkey,
    config: &Config,
    policy: &CollectionPolicy,
    owner: &Pubkey,
    consent: &CollectionConsent,
    report: &CollectionReport,
    now: i64,
) -> Result<u64> {
    let info = info.ok_or(EndowmentError::InvalidCollection)?;
    let (key, bump) = Pubkey::find_program_address(
        &[PERMIT_SEED, config_key.as_ref(), owner.as_ref()],
        &crate::ID,
    );
    require_keys_eq!(info.key(), key, EndowmentError::InvalidCollection);
    require_keys_eq!(*info.owner, crate::ID, EndowmentError::InvalidCollection);
    let permit = CollectionPermit::try_deserialize(&mut info.try_borrow_data()?.as_ref())?;
    require!(
        permit.bump == bump
            && permit.config == *config_key
            && permit.owner == *owner
            && permit.collector == policy.collector
            && permit.reviewer == policy.reviewer
            && CollectionPolicy::valid_roles(&permit.collector, &permit.reviewer)
            && permit.consent_epoch == consent.epoch
            && permit.nonce == consent.next_nonce
            && permit.expected_balance == report.expected_balance
            && permit.amount >= report.amount
            && permit.valid_until >= now
            && report.valid_until <= permit.valid_until
            && permit.count_at == config.last_count_at
            && permit.refresher_epoch == config.refresher_epoch
            && permit.pause_started_at == config.pause_started_at
            && permit.evidence_hash != [0; 32],
        EndowmentError::InvalidCollection
    );
    Ok(permit.amount)
}
