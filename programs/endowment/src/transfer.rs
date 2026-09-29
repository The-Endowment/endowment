//! PUMP transfers that keep working if PUMP's transfer hook is ever switched on.
//!
//! The PUMP mint carries a Token-2022 `TransferHook` extension whose program is
//! currently unset. While it is unset, a transfer is a plain `transfer_checked`.
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
    extension::{transfer_hook::TransferHook, BaseStateWithExtensions, StateWithExtensions},
    state::Mint as MintState,
};

/// Token-2022 `TransferChecked` instruction tag.
const TRANSFER_CHECKED: u8 = 12;

/// True when `mint` has a transfer-hook program set.
pub fn hook_enabled(mint: &AccountInfo) -> Result<bool> {
    let data = mint.try_borrow_data()?;
    let state = StateWithExtensions::<MintState>::unpack(&data)?;
    Ok(match state.get_extension::<TransferHook>() {
        // An unset hook program is stored as all zeroes.
        Ok(hook) => hook.program_id.0.as_ref().iter().any(|b| *b != 0),
        Err(_) => false,
    })
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
