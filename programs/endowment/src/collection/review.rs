use super::state::*;
use crate::{constants::*, error::EndowmentError, state::Config};
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct ReviewCollection<'info> {
    #[account(address = policy.reviewer @ EndowmentError::NotReviewer)]
    pub reviewer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [POLICY_SEED, config.key().as_ref()], bump = policy.bump, has_one = config)]
    pub policy: Box<Account<'info, CollectionPolicy>>,
    #[account(mut, seeds = [RECEIPT_SEED, config.key().as_ref(), receipt.owner.as_ref(), &receipt.nonce.to_le_bytes()],
        bump = receipt.bump, has_one = config @ EndowmentError::InvalidCollection)]
    pub receipt: Box<Account<'info, PendingCollection>>,
}
pub fn review(ctx: Context<ReviewCollection>, approved_amount: u64, evidence_hash: [u8; 32]) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let deadline = ctx.accounts.receipt.refund_at;
    let receipt = &mut ctx.accounts.receipt;
    // Nobody reviews what it collected itself, whatever role changes happened
    // since (the receipt's rent payer is the collector that signed it).
    require_keys_neq!(ctx.accounts.reviewer.key(), receipt.payer, EndowmentError::NotReviewer);
    require!(now >= receipt.release_at, EndowmentError::HoldNotElapsed);
    require!(now < deadline, EndowmentError::CollectionExpired);
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
