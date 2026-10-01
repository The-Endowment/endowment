use super::state::*;
use crate::error::EndowmentError;
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct ReviewCollection<'info> {
    #[account(address = policy.reviewer @ EndowmentError::NotReviewer)]
    pub reviewer: Signer<'info>,
    #[account(seeds = [POLICY_SEED, policy.config.as_ref()], bump = policy.bump)]
    pub policy: Box<Account<'info, CollectionPolicy>>,
    #[account(mut, seeds = [RECEIPT_SEED, receipt.config.as_ref(), receipt.owner.as_ref(), &receipt.nonce.to_le_bytes()],
        bump = receipt.bump, constraint = receipt.config == policy.config @ EndowmentError::InvalidCollection)]
    pub receipt: Box<Account<'info, PendingCollection>>,
}
pub fn review(ctx: Context<ReviewCollection>, approved_amount: u64, evidence_hash: [u8; 32]) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let receipt = &mut ctx.accounts.receipt;
    require!(now >= receipt.release_at, EndowmentError::HoldNotElapsed);
    require!(now < receipt.refund_at, EndowmentError::CollectionExpired);
    require!(
        !receipt.reviewed && approved_amount <= receipt.amount && evidence_hash != [0; 32],
        EndowmentError::InvalidCollection
    );
    receipt.reviewed = true;
    receipt.approved_amount = approved_amount;
    receipt.review_evidence = evidence_hash;
    emit!(CollectionReviewed {
        receipt: receipt.key(),
        approved_amount,
        evidence_hash
    });
    Ok(())
}
