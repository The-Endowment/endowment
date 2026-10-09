use crate::error::EndowmentError;
use anchor_lang::prelude::*;

pub const POLICY_SEED: &[u8] = b"collection_policy";
pub const CONSENT_SEED: &[u8] = b"collection_consent";
pub const RECEIPT_SEED: &[u8] = b"pending_collection";
pub const HOLD_SECONDS: i64 = 86_400;
pub const REFUND_SECONDS: i64 = 3 * HOLD_SECONDS;
pub const REPORT_SECONDS: i64 = 60;

/// The collector and reviewer, and the totals of what was collected. The policy
/// PDA owns the holding account, separate from the buyback authority's vault.
/// No instruction can shorten a hold. The roles change only through
/// `propose_collection_roles` → 72h → `apply_collection_roles`, by the admin,
/// and are fixed for good once the admin renounces.
#[account]
#[derive(InitSpace)]
pub struct CollectionPolicy {
    pub config: Pubkey,
    pub collector: Pubkey,
    pub reviewer: Pubkey,
    pub bump: u8,
    pub pending: u64,
    pub released: u64,
    pub refunded: u64,
    /// A role change waiting out the timelock; `pending_roles_at` 0 = none.
    pub pending_collector: Pubkey,
    pub pending_reviewer: Pubkey,
    pub pending_roles_at: i64,
}

impl CollectionPolicy {
    pub fn valid_roles(collector: &Pubkey, reviewer: &Pubkey) -> bool {
        *collector != Pubkey::default() && *reviewer != Pubkey::default() && collector != reviewer
    }
}

/// Never closed, including on deregistration. Nonces and a holder's decision
/// to stop survive leaving, re-enrollment, and token-account closure.
#[account]
#[derive(InitSpace)]
pub struct CollectionConsent {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub bump: u8,
    pub enabled: bool,
    pub epoch: u64,
    pub next_nonce: u64,
    pub started_at: i64,
}
impl CollectionConsent {
    pub fn disable(&mut self) -> Result<()> {
        self.enabled = false;
        self.epoch = self.epoch.checked_add(1).ok_or(EndowmentError::Overflow)?;
        Ok(())
    }
}

#[account]
#[derive(InitSpace)]
pub struct PendingCollection {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub payer: Pubkey,
    pub nonce: u64,
    pub consent_epoch: u64,
    pub amount: u64,
    pub collected_at: i64,
    pub release_at: i64,
    pub refund_at: i64,
    pub collection_evidence: [u8; 32],
    pub reviewed: bool,
    pub approved_amount: u64,
    pub review_evidence: [u8; 32],
    pub bump: u8,
}

impl PendingCollection {
    /// A pause cancels all still-pending contributions collected through that
    /// second, including approved receipts. Resume never restores them.
    pub fn invalidated_by_pause(&self, pause_started_at: i64) -> bool {
        pause_started_at > 0 && self.collected_at <= pause_started_at
    }
}

/// Exact, short-lived authorization, tied to one consent generation and nonce.
/// A balance check catches intervening changes, but not a same-balance
/// spend/rebuy. Independent review must replay the finalized history as well.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct CollectionReport {
    pub consent_epoch: u64,
    pub expected_balance: u64,
    pub amount: u64,
    pub valid_until: i64,
    pub evidence_hash: [u8; 32],
}

#[event]
pub struct CollectionHeld {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub nonce: u64,
    pub amount: u64,
    pub release_at: i64,
    pub refund_at: i64,
    pub evidence_hash: [u8; 32],
}
#[event]
pub struct CollectionReviewed {
    pub receipt: Pubkey,
    pub approved_amount: u64,
    pub evidence_hash: [u8; 32],
}
#[event]
pub struct CollectionSettled {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub nonce: u64,
    pub released: u64,
    pub refunded: u64,
}
#[event]
pub struct CollectionRolesProposed {
    pub config: Pubkey,
    pub collector: Pubkey,
    pub reviewer: Pubkey,
    pub effective_at: i64,
}
#[event]
pub struct CollectionRolesChanged {
    pub config: Pubkey,
    pub collector: Pubkey,
    pub reviewer: Pubkey,
}
