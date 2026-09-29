use anchor_lang::prelude::*;

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
}

#[event]
pub struct LandlordDeregistered {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub total_contributed: u64,
}

#[event]
pub struct Swept {
    pub config: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    pub total_contributed: u64,
}

#[event]
pub struct Bought {
    pub config: Pubkey,
    pub dividend_in: u64,
    pub coin_out: u64,
    pub min_acceptable: u64,
    /// Dividend routed to permanent liquidity (post-close only).
    pub liquidity_dividend: u64,
    pub lp_tokens: u64,
    pub tip: u64,
    /// Sent to the flagship endowment's dividend vault.
    pub donation: u64,
    pub total_dividend_spent: u64,
    pub total_coin_bought: u64,
}

#[event]
pub struct BuybackLimitsChanged {
    pub config: Pubkey,
    pub max_buy_per_tx: u64,
    pub max_buy_per_day: u64,
    pub max_price_impact_bps: u16,
}

#[event]
pub struct BuyParamsChanged {
    pub config: Pubkey,
    pub buy_bps: u16,
    pub min_buy_interval_secs: i64,
    pub tip_bps: u16,
}

#[event]
pub struct ActivationChanged {
    pub config: Pubkey,
    pub activate_bps: u16,
    pub deactivate_bps: u16,
    pub active: bool,
}

#[event]
pub struct CountStarted {
    pub config: Pubkey,
    pub round: u64,
    pub expected: u32,
}

#[event]
pub struct CountFinished {
    pub config: Pubkey,
    pub round: u64,
    pub committed: u64,
    pub committed_bps: u16,
    pub active: bool,
}

#[event]
pub struct ContributionsClosed {
    pub config: Pubkey,
    /// True when the cap was reached; false when the admin retired contributions early.
    pub by_cap: bool,
    pub coin_held: u64,
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
