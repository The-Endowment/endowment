//! Dividend transfers that keep working if the dividend mint's transfer hook is
//! ever switched on.
//!
//! A Token-2022 dividend mint may carry a `TransferHook` extension (PUMP's
//! program is currently unset). While it is unset, a transfer is a plain `transfer_checked`.
//! If it is ever set, Token-2022 calls the hook program during every transfer and
//! needs the hook's extra accounts in the instruction: the hook program, its
//! `extra-account-metas` validation account, and whatever that account lists.
//! Callers resolve those off-chain (for example with
//! `addExtraAccountMetasForExecute` from `@solana/spl-token`) and pass them as the
//! instruction's remaining accounts; this helper forwards them unchanged to
//! Token-2022, which validates them against the hook's own account list.

use anchor_lang::{
    prelude::*,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        program::invoke_signed,
    },
};
use anchor_spl::token_2022::spl_token_2022::{
    extension::{
        transfer_fee::TransferFeeConfig, transfer_hook::TransferHook, BaseStateWithExtensions, StateWithExtensions,
    },
    state::Mint as MintState,
};

use crate::{constants::MAX_TRANSFER_FEE_BPS, error::EndowmentError};

/// Token-2022 `TransferChecked` instruction tag.
const TRANSFER_CHECKED: u8 = 12;

/// True when `mint` has a transfer-hook program set. Original SPL Token mints
/// have no extensions.
pub fn hook_enabled(mint: &AccountInfo) -> Result<bool> {
    let data = mint.try_borrow_data()?;
    let state = StateWithExtensions::<MintState>::unpack(&data)?;
    Ok(match state.get_extension::<TransferHook>() {
        // An unset hook program is stored as all zeroes.
        Ok(hook) => hook.program_id.0.as_ref().iter().any(|b| *b != 0),
        Err(_) => false,
    })
}

/// A mint's transfer fee for this epoch, in basis points, failing closed if
/// either the current or the scheduled (newer) fee is above the hard ceiling.
/// A fee authority can schedule a raise two epochs ahead; refusing as soon as
/// one is scheduled means a raised fee never reaches a buyback.
pub fn capped_transfer_fee_bps(mint: &AccountInfo, epoch: u64) -> Result<u64> {
    let data = mint.try_borrow_data()?;
    let state = StateWithExtensions::<MintState>::unpack(&data)?;
    let Ok(config) = state.get_extension::<TransferFeeConfig>() else {
        return Ok(0);
    };
    let older = u16::from(config.older_transfer_fee.transfer_fee_basis_points) as u64;
    let newer = u16::from(config.newer_transfer_fee.transfer_fee_basis_points) as u64;
    require!(
        older <= MAX_TRANSFER_FEE_BPS && newer <= MAX_TRANSFER_FEE_BPS,
        EndowmentError::FeeTooHigh
    );
    Ok(u16::from(config.get_epoch_fee(epoch).transfer_fee_basis_points) as u64)
}

/// The fields of a token account (SPL Token or Token-2022; both share this base
/// layout) that counting and pruning need.
pub struct TokenView {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    pub delegate: Option<Pubkey>,
    pub delegated_amount: u64,
    pub close_authority: Option<Pubkey>,
}

impl TokenView {
    /// Whether this account delegates at least `MIN_DELEGATION` to `authority`.
    pub fn delegates_to(&self, authority: &Pubkey) -> bool {
        self.delegate == Some(*authority) && self.delegated_amount >= crate::constants::MIN_DELEGATION
    }
}

/// Reads a token account, or `None` if it's closed (a closed account holds
/// nothing and delegates nothing). Any live account must belong to a token program.
pub fn read_token_account(info: &AccountInfo) -> Result<Option<TokenView>> {
    if info.data_is_empty() {
        return Ok(None);
    }
    require!(
        *info.owner == anchor_spl::token::ID || *info.owner == anchor_spl::token_2022::ID,
        EndowmentError::InvalidCountAccount
    );
    let data = info.try_borrow_data()?;
    require!(data.len() >= 165, EndowmentError::InvalidCountAccount);
    let key = |at: usize| Pubkey::new_from_array(data[at..at + 32].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(data[at..at + 8].try_into().unwrap());
    let tag = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    Ok(Some(TokenView {
        mint: key(0),
        owner: key(32),
        amount: u64_at(64),
        delegate: if tag(72) == 1 { Some(key(76)) } else { None },
        delegated_amount: u64_at(121),
        close_authority: if tag(129) == 1 { Some(key(133)) } else { None },
    }))
}

#[allow(clippy::too_many_arguments)]
pub fn transfer_checked_with_hook<'info>(
    token_program: &AccountInfo<'info>,
    from: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    hook_accounts: &[AccountInfo<'info>],
    amount: u64,
    decimals: u8,
    signer: &[&[&[u8]]],
) -> Result<()> {
    let extras: &[AccountInfo<'info>] = if hook_enabled(mint)? { hook_accounts } else { &[] };

    let mut data = Vec::with_capacity(10);
    data.push(TRANSFER_CHECKED);
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);

    let mut metas = vec![
        AccountMeta::new(from.key(), false),
        AccountMeta::new_readonly(mint.key(), false),
        AccountMeta::new(to.key(), false),
        AccountMeta::new_readonly(authority.key(), true),
    ];
    // Extra accounts are forwarded without signer privileges.
    metas.extend(extras.iter().map(|a| {
        if a.is_writable {
            AccountMeta::new(a.key(), false)
        } else {
            AccountMeta::new_readonly(a.key(), false)
        }
    }));
    let ix = Instruction { program_id: token_program.key(), accounts: metas, data };

    let mut infos = vec![from.clone(), mint.clone(), to.clone(), authority.clone()];
    infos.extend(extras.iter().cloned());
    infos.push(token_program.clone());
    invoke_signed(&ix, &infos, signer)?;
    Ok(())
}
