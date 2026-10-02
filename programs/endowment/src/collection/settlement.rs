use super::state::*;
use crate::{
    constants::*,
    error::EndowmentError,
    state::{Config, Landlord},
};
use anchor_lang::prelude::*;
use anchor_lang::solana_program::{instruction::Instruction, program::invoke};
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked},
};

/// The only exits from pending custody: its fixed treasury or the original
/// holder's canonical ATA. Neither caller nor reviewer can nominate a recipient.
///
/// Remaining accounts: optionally the SPL Memo program, first. With it, a
/// memo precedes the refund, so a holder whose account requires memos on
/// incoming transfers can't make its receipts unsettleable. The endowment's
/// own services always pass it.
///
/// Only the holder's own reclaim of a current receipt switches its collection
/// off. Every refund raises the holder's baseline by what came back, so the
/// refunded dividend is never collected again, whoever executed the refund.
#[derive(Accounts)]
pub struct SettleCollection<'info> {
    #[account(mut)]
    pub caller: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [POLICY_SEED, config.key().as_ref()], bump = policy.bump, has_one = config)]
    pub policy: Box<Account<'info, CollectionPolicy>>,
    #[account(mut, close = rent_recipient,
        seeds = [RECEIPT_SEED, config.key().as_ref(), receipt.owner.as_ref(), &receipt.nonce.to_le_bytes()],
        bump = receipt.bump, has_one = config)]
    pub receipt: Box<Account<'info, PendingCollection>>,
    #[account(mut, seeds = [CONSENT_SEED, config.key().as_ref(), receipt.owner.as_ref()], bump = consent.bump,
        has_one = config, constraint = consent.owner == receipt.owner @ EndowmentError::InvalidCollection)]
    pub consent: Box<Account<'info, CollectionConsent>>,
    /// CHECK: only receives the rent paid when this receipt was created.
    #[account(mut, address = receipt.payer)]
    pub rent_recipient: UncheckedAccount<'info>,
    /// CHECK: original holder, used as the authority of their canonical refund ATA.
    #[account(address = receipt.owner)]
    pub owner: UncheckedAccount<'info>,
    /// CHECK: the holder's landlord record at its derived address, if it still
    /// exists (it is closed on leaving). Its address is checked, and it is
    /// read and written, in `protect_refund` (kept out of this struct's
    /// validation for stack space).
    #[account(mut)]
    pub landlord: UncheckedAccount<'info>,
    #[account(address = config.dividend_mint)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, associated_token::mint = dividend_mint, associated_token::authority = policy,
        associated_token::token_program = dividend_token_program)]
    pub pending_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(init_if_needed, payer = caller, associated_token::mint = dividend_mint,
        associated_token::authority = owner, associated_token::token_program = dividend_token_program)]
    pub refund_account: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the existing buyback authority, unrelated to pending custody.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(mut, associated_token::mint = dividend_mint, associated_token::authority = authority,
        associated_token::token_program = dividend_token_program)]
    pub dividend_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(associated_token::mint = coin_mint, associated_token::authority = authority,
        associated_token::token_program = coin_token_program)]
    pub coin_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub coin_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn settle<'info>(ctx: Context<'_, SettleCollection<'info>>, release: bool) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let memo_program = ctx.remaining_accounts.first().cloned();
    let a = ctx.accounts;
    let config_key = a.config.key();
    // A holder that left or was pruned has no record: nothing of its is released.
    let enrolled = *a.landlord.owner == crate::ID && !a.landlord.data_is_empty();
    let goal = a.config.record_completion(config_key, a.coin_vault.amount);
    let current_consent = a.consent.enabled && a.consent.epoch == a.receipt.consent_epoch;
    // A pause doesn't run out the time to review (`PendingCollection::deadline`).
    let deadline = a.receipt.deadline(a.config.pause_started_at, a.config.paused_until);
    let reclaimed = !release && a.caller.key() == a.receipt.owner;
    let released = if release {
        require!(!a.config.is_paused(now), EndowmentError::Paused);
        require!(!a.config.retired && !goal, EndowmentError::Completed);
        require!(current_consent && enrolled, EndowmentError::CollectionConsentRequired);
        require!(now >= a.receipt.release_at, EndowmentError::HoldNotElapsed);
        require!(now < deadline, EndowmentError::CollectionExpired);
        require!(a.receipt.reviewed, EndowmentError::CollectionNotReviewed);
        a.receipt.approved_amount
    } else {
        require!(
            reclaimed
                || a.caller.key() == a.policy.reviewer
                || now >= deadline
                || !current_consent
                || !enrolled
                || goal
                || a.config.retired,
            EndowmentError::RefundNotAllowed
        );
        0
    };
    let refunded = a.receipt.amount.checked_sub(released).ok_or(EndowmentError::Overflow)?;
    protect_refund(&a.landlord, &config_key, &a.receipt.owner, a.receipt.nonce, refunded)?;
    // All receipts remain backed, even if someone transfers unsolicited tokens
    // into the pending vault. Unattributed donations create no withdrawal right.
    require!(
        a.pending_vault.amount >= a.policy.pending,
        EndowmentError::InvalidCollection
    );
    let bump = [a.policy.bump];
    let seeds: &[&[u8]] = &[POLICY_SEED, config_key.as_ref(), &bump];
    let transfer = |amount: u64, to: AccountInfo<'info>| -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                a.dividend_token_program.key(),
                TransferChecked {
                    from: a.pending_vault.to_account_info(),
                    mint: a.dividend_mint.to_account_info(),
                    to,
                    authority: a.policy.to_account_info(),
                },
                &[seeds],
            ),
            amount,
            a.dividend_mint.decimals,
        )
    };
    let treasury_before = a.dividend_vault.amount;
    let refund_before = a.refund_account.amount;
    transfer(released, a.dividend_vault.to_account_info())?;
    if refunded > 0 {
        refund_memo(memo_program)?;
    }
    transfer(refunded, a.refund_account.to_account_info())?;
    a.dividend_vault.reload()?;
    a.refund_account.reload()?;
    require!(
        a.dividend_vault.amount.checked_sub(treasury_before) == Some(released)
            && a.refund_account.amount.checked_sub(refund_before) == Some(refunded),
        EndowmentError::UnsupportedRefundMint
    );
    a.policy.pending = a
        .policy
        .pending
        .checked_sub(a.receipt.amount)
        .ok_or(EndowmentError::Overflow)?;
    a.policy.released = a
        .policy
        .released
        .checked_add(released)
        .ok_or(EndowmentError::Overflow)?;
    a.policy.refunded = a
        .policy
        .refunded
        .checked_add(refunded)
        .ok_or(EndowmentError::Overflow)?;
    // The holder asked for its dividend back: collection stops until it
    // consents again. A refund by anyone else, or of a receipt from an earlier
    // consent, leaves its consent as it is.
    if reclaimed && current_consent {
        a.consent.disable()?;
    }
    if refunded > 0 {
        // The totals count what was collected and not sent back.
        a.config.total_swept = a.config.total_swept.saturating_sub(refunded);
    }
    emit!(CollectionSettled {
        config: config_key,
        owner: a.receipt.owner,
        nonce: a.receipt.nonce,
        released,
        refunded
    });
    Ok(())
}

/// Issues a memo, if the Memo program was passed: Token-2022 accepts a
/// transfer into a memo-requiring account when a memo was the instruction
/// just before it.
#[inline(never)]
fn refund_memo(memo_program: Option<AccountInfo>) -> Result<()> {
    let Some(program) = memo_program else {
        return Ok(());
    };
    require_keys_eq!(program.key(), MEMO_PROGRAM_ID, EndowmentError::InvalidCollection);
    invoke(
        &Instruction { program_id: MEMO_PROGRAM_ID, accounts: vec![], data: b"refund".to_vec() },
        &[program],
    )?;
    Ok(())
}

/// Raises the holder's baseline by a refund, so what came back stays theirs,
/// and takes it out of their contribution total. A holder that has left has no
/// record (and nothing can be collected from it); rejoining sets a new baseline.
#[inline(never)]
fn protect_refund(info: &UncheckedAccount, config: &Pubkey, owner: &Pubkey, nonce: u64, refunded: u64) -> Result<()> {
    let (expected, _) = Pubkey::find_program_address(&[LANDLORD_SEED, config.as_ref(), owner.as_ref()], &crate::ID);
    require_keys_eq!(info.key(), expected, EndowmentError::InvalidCollection);
    if refunded == 0 || *info.owner != crate::ID || info.data_is_empty() {
        return Ok(());
    }
    let mut landlord = Landlord::try_deserialize(&mut &info.try_borrow_data()?[..])?;
    landlord.baseline = landlord.baseline.saturating_add(refunded);
    // Protect every refund, but only undo totals charged to this registration.
    if nonce >= landlord.first_collection_nonce {
        landlord.total_contributed = landlord.total_contributed.saturating_sub(refunded);
    }
    landlord.try_serialize(&mut &mut info.try_borrow_mut_data()?[..])?;
    Ok(())
}
