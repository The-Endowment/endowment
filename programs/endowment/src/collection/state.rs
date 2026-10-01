use crate::error::EndowmentError;
use anchor_lang::prelude::*;

pub const POLICY_SEED: &[u8] = b"collection_policy";
pub const CONSENT_SEED: &[u8] = b"collection_consent";
pub const RECEIPT_SEED: &[u8] = b"pending_collection";
pub const HOLD_SECONDS: i64 = 86_400;
pub const REFUND_SECONDS: i64 = 3 * HOLD_SECONDS;
pub const REPORT_SECONDS: i64 = 60;

/// Immutable role addresses; no instruction can replace them or shorten a hold.
/// The policy PDA owns an ATA separate from the buyback authority's ATA.
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
}

/// Never closed, including on deregistration. Nonces and refund revocation
/// survive leaving, re-enrollment, and token-account closure/recreation.
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
