//! Explicit reporter authorizations. The signature attests provenance; the
//! program verifies bounds, never infers rewards from a balance increase.
use crate::{
    constants::*,
    error::EndowmentError,
    state::{Config, Landlord},
};
use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct ReporterPolicy {
    pub config: Pubkey,
    pub reporter: Pubkey,
    pub pending: Pubkey,
    pub effective_at: i64,
    pub epoch: u64,
    pub disabled: bool,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct RewardReport {
    pub consent_id: u64,
    pub nonce: u64,
    pub collection_epoch: u64,
    pub reporter_epoch: u64,
    pub amount: u64,
    pub expected_source_balance: u64,
    pub issued_at: i64,
    pub expires_at: i64,
    pub evidence_hash: [u8; 32],
}

impl RewardReport {
    pub fn validate(
        &self,
        config: &Config,
        landlord: &Landlord,
        policy: &ReporterPolicy,
        now: i64,
    ) -> Result<()> {
        require!(
            config.version == CONFIG_VERSION && landlord.version == LANDLORD_VERSION,
            EndowmentError::UnsupportedCollectionVersion
        );
        require!(
            self.consent_id == landlord.consent_id
                && self.consent_id != 0
                && self.collection_epoch == config.collection_epoch
                && config.collection_epoch != u64::MAX
                && self.reporter_epoch == policy.epoch,
            EndowmentError::StaleReport
        );
        require!(
            Some(self.nonce) == landlord.last_report_nonce.checked_add(1),
            EndowmentError::ReportReplay
        );
        require!(
            self.issued_at <= now
                && now <= self.expires_at
                && self
                    .expires_at
                    .checked_sub(self.issued_at)
                    .is_some_and(|ttl| (0..=MAX_REPORT_LIFETIME_SECS).contains(&ttl)),
            EndowmentError::ReportExpired
        );
        require!(self.amount > 0, EndowmentError::CollectionLimit);
        Ok(())
    }
}

impl ReporterPolicy {
    pub fn invalidate(&mut self) -> Result<()> {
        self.epoch = self.epoch.checked_add(1).ok_or(EndowmentError::Overflow)?;
        Ok(())
    }
    pub fn emit_change(&self) {
        emit!(crate::events::ReporterChanged {
            config: self.config,
            reporter: self.reporter,
            pending: self.pending,
            effective_at: self.effective_at,
            epoch: self.epoch,
            disabled: self.disabled,
        });
    }
}
