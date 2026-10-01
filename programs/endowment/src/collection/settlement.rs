use super::state::*;
use crate::{constants::*, error::EndowmentError, state::Config};
use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked},
};

/// The only exits from pending custody: its fixed treasury or the original
/// holder's canonical ATA. Neither caller nor reviewer can nominate a recipient.
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
    let a = ctx.accounts;
    let config_key = a.config.key();
    let goal = a.config.record_completion(config_key, a.coin_vault.amount);
    let current_consent = a.consent.enabled && a.consent.epoch == a.receipt.consent_epoch;
    let released = if release {
        require!(!a.config.is_paused(now), EndowmentError::Paused);
        require!(!a.config.retired && !goal, EndowmentError::Completed);
        require!(current_consent, EndowmentError::CollectionConsentRequired);
        require!(now >= a.receipt.release_at, EndowmentError::HoldNotElapsed);
        require!(now < a.receipt.refund_at, EndowmentError::CollectionExpired);
        require!(a.receipt.reviewed, EndowmentError::CollectionNotReviewed);
        a.receipt.approved_amount
    } else {
        require!(
            a.caller.key() == a.receipt.owner
                || a.caller.key() == a.policy.reviewer
                || now >= a.receipt.refund_at
                || !current_consent
                || goal
                || a.config.retired,
            EndowmentError::RefundNotAllowed
        );
        0
    };
    let refunded = a.receipt.amount.checked_sub(released).ok_or(EndowmentError::Overflow)?;
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
    if refunded > 0 {
        a.consent.disable()?;
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
