use anchor_lang::prelude::*;

use crate::state::Params;

// Every event names the endowment instance (`config`) it belongs to, so one
// indexer can follow every endowment on the shared contract.

#[event]
pub struct EndowmentCreated {
    pub config: Pubkey,
    pub creator: Pubkey,
    pub coin_mint: Pubkey,
    pub dividend_mint: Pubkey,
    pub pool: Pubkey,
    pub donation_bps: u16,
}

#[event]
pub struct LandlordRegistered {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub baseline: u64,
    pub coin_held: u64,
}

#[event]
pub struct LandlordDeregistered {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub total_contributed: u64,
}

/// A landlord removed by someone else because it no longer qualifies: it revoked
/// its delegation or fell below the minimum stake.
#[event]
pub struct LandlordRemoved {
    pub config: Pubkey,
    pub owner: Pubkey,
}

#[event]
pub struct BaselineChanged {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub old_baseline: u64,
    pub new_baseline: u64,
}

#[event]
pub struct Swept {
    pub config: Pubkey,
    pub owner: Pubkey,
    /// What left the landlord's account.
    pub amount: u64,
    /// What arrived in the vault (less any transfer fee).
    pub received: u64,
    pub baseline: u64,
    pub total_contributed: u64,
}

#[event]
pub struct Bought {
    pub config: Pubkey,
    /// Dividend that left the vault for the swap and any liquidity deposit.
    pub dividend_spent: u64,
    pub coin_out: u64,
    pub min_acceptable: u64,
    /// Coin-per-dividend TWAP the floor was measured against (Q32.32).
    pub twap_price_x32: u128,
    /// Dividend and coin deposited as permanent liquidity (after the milestone).
    pub liquidity_dividend: u64,
    pub liquidity_coin: u64,
    pub lp_tokens: u64,
    pub tip: u64,
    /// Sent to the flagship endowment's dividend vault.
    pub donation: u64,
    pub total_dividend_spent: u64,
    pub total_coin_bought: u64,
}

#[event]
pub struct MilestoneReached {
    pub config: Pubkey,
    pub total_coin_bought: u64,
}

#[event]
pub struct CountStarted {
    pub config: Pubkey,
    pub round: u64,
    pub expected: u32,
    pub supply: u64,
}

/// One landlord's line in the public tally. `raw_balance` is its coin balance at
/// the moment it was read; comparing it round to round shows coin moving between
/// landlord wallets.
#[event]
pub struct LandlordCounted {
    pub config: Pubkey,
    pub round: u64,
    pub landlord: Pubkey,
    pub owner: Pubkey,
    pub counted: u64,
    pub raw_balance: u64,
}

#[event]
pub struct CommitmentCounted {
    pub config: Pubkey,
    pub round: u64,
    pub expected: u32,
    pub counted: u32,
    pub committed: u64,
    pub committed_bps: u16,
    pub active: bool,
    /// The round was finished after its timeout, with some landlords uncounted.
    pub timed_out: bool,
}

#[event]
pub struct ParamsProposed {
    pub config: Pubkey,
    pub params: Params,
    pub effective_at: i64,
}

#[event]
pub struct ParamsApplied {
    pub config: Pubkey,
    pub params: Params,
    pub active: bool,
}

#[event]
pub struct ParamsCancelled {
    pub config: Pubkey,
}

#[event]
pub struct Retired {
    pub config: Pubkey,
}

#[event]
pub struct PauseChanged {
    pub config: Pubkey,
    pub paused_until: i64,
}

#[event]
pub struct GuardianChanged {
    pub config: Pubkey,
    pub guardian: Pubkey,
}

#[event]
pub struct AdminProposed {
    pub config: Pubkey,
    pub pending_admin: Pubkey,
}

#[event]
pub struct AdminAccepted {
    pub config: Pubkey,
    pub admin: Pubkey,
}

#[event]
pub struct AdminRenounced {
    pub config: Pubkey,
}
