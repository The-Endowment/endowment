//! Which mints an endowment can be created for.
//!
//! Creation is permissionless, so these checks are what every endowment on the
//! shared contract can rely on, whoever created it.
//!
//! The coin (held forever) may have no authority that could take it back, stop
//! it moving, or dilute the count it's measured against:
//! - no mint authority and no freeze authority;
//! - no permanent delegate, pausable, non-transferable, confidential-transfer
//!   or confidential mint/burn extension, and no frozen default account state;
//! - no transfer-hook program set (a hook authority alone is allowed: sweeps
//!   and buybacks stop while a hook is set, see `health`).
//! A transfer fee is allowed (buybacks refuse fees above the cap), as are
//! metadata, groups, interest-bearing and scaled-UI-amount displays.
//!
//! The dividend (spent, never held) needs only to keep moving: no permanent
//! delegate, pausable, non-transferable or confidential extension, no frozen
//! default state and no hook program set. A freeze authority is allowed
//! (frozen vaults stop sweeps and buybacks, see `health`), as is a transfer fee
//! within the cap.
//!
//! Both of the flagship's mints pass: $PENIS has no authorities and a 3%
//! transfer fee; PUMP has no authorities and a transfer-hook authority with no
//! program set.

use anchor_lang::prelude::*;
use anchor_spl::token_2022::spl_token_2022::{
    extension::{
        default_account_state::DefaultAccountState, transfer_hook::TransferHook, BaseStateWithExtensions,
        ExtensionType, StateWithExtensions,
    },
    state::{AccountState, Mint as MintState},
};

use crate::error::EndowmentError;

/// Extensions neither mint may carry.
const FORBIDDEN: [ExtensionType; 6] = [
    ExtensionType::PermanentDelegate,
    ExtensionType::Pausable,
    ExtensionType::NonTransferable,
    ExtensionType::ConfidentialTransferMint,
    ExtensionType::ConfidentialTransferFeeConfig,
    ExtensionType::ConfidentialMintBurn,
];

pub fn check_coin_mint(mint: &AccountInfo) -> Result<()> {
    check(mint, true)
}

pub fn check_dividend_mint(mint: &AccountInfo) -> Result<()> {
    check(mint, false)
}

fn check(mint: &AccountInfo, is_coin: bool) -> Result<()> {
    let data = mint.try_borrow_data()?;
    // Original SPL Token mints have no extensions; the base layout is shared.
    let state = StateWithExtensions::<MintState>::unpack(&data).map_err(|_| error!(EndowmentError::UnsafeMint))?;
    if is_coin {
        require!(
            state.base.mint_authority.is_none() && state.base.freeze_authority.is_none(),
            EndowmentError::UnsafeMint
        );
    }
    let types = state.get_extension_types().map_err(|_| error!(EndowmentError::UnsafeMint))?;
    require!(!types.iter().any(|t| FORBIDDEN.contains(t)), EndowmentError::UnsafeMint);
    if let Ok(default) = state.get_extension::<DefaultAccountState>() {
        require!(default.state != AccountState::Frozen as u8, EndowmentError::UnsafeMint);
    }
    if let Ok(hook) = state.get_extension::<TransferHook>() {
        let program: Option<Pubkey> = hook.program_id.into();
        require!(program.is_none(), EndowmentError::UnsafeMint);
    }
    Ok(())
}
