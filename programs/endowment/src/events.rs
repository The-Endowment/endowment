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
pub struct PauseChanged {
    pub paused_until: i64,
}
