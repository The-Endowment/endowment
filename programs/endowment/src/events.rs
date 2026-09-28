use anchor_lang::prelude::*;

#[event]
pub struct LandlordRegistered {
    pub owner: Pubkey,
    pub baseline: u64,
}

#[event]
pub struct LandlordDeregistered {
    pub owner: Pubkey,
    pub total_contributed: u64,
}

#[event]
pub struct Swept {
    pub owner: Pubkey,
    pub amount: u64,
    pub total_contributed: u64,
}

#[event]
pub struct Bought {
    pub pump_in: u64,
    pub penis_out: u64,
    pub min_acceptable: u64,
    pub total_pump_spent: u64,
    pub total_penis_bought: u64,
}

#[event]
pub struct BuybackLimitsChanged {
    pub max_buy_per_tx: u64,
    pub max_buy_per_day: u64,
    pub max_price_impact_bps: u16,
}

#[event]
pub struct PauseChanged {
    pub paused_until: i64,
}

#[event]
pub struct GuardianChanged {
    pub guardian: Pubkey,
}

#[event]
pub struct AdminProposed {
    pub pending_admin: Pubkey,
}

#[event]
pub struct AdminAccepted {
    pub admin: Pubkey,
}
