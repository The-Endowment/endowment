use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct Config {
    /// Proposes bounded parameter changes.
    pub admin: Pubkey,
    /// Proposed next admin; must sign `accept_admin`. Default = none.
    pub pending_admin: Pubkey,
    /// Can pause cranks for at most MAX_PAUSE_SECONDS. Cannot move funds.
    pub guardian: Pubkey,
    pub pump_mint: Pubkey,
    pub penis_mint: Pubkey,
    /// Accumulation target as basis points of $PENIS supply.
    pub supply_target_bps: u16,
    /// Unix timestamp; cranks are blocked while `now < paused_until`.
    pub paused_until: i64,
    pub total_swept: u64,
    pub landlord_count: u32,
    pub bump: u8,
    pub authority_bump: u8,

    /// The Raydium CPMM PENIS/PUMP pool buybacks trade against.
    pub pool: Pubkey,
    /// Buyback limits, in PUMP base units, bounded by `validate_limits`.
    pub max_buy_per_tx: u64,
    pub max_buy_per_day: u64,
    /// Slippage a single buyback may cause, beyond fees.
    pub max_price_impact_bps: u16,
    /// Rolling 24-hour buyback window.
    pub day_start: i64,
    pub bought_today: u64,
    pub total_pump_spent: u64,
    pub total_penis_bought: u64,
}

impl Config {
    pub fn is_paused(&self, now: i64) -> bool {
        now < self.paused_until
    }
}

pub fn validate_limits(max_buy_per_tx: u64, max_buy_per_day: u64, max_price_impact_bps: u16) -> Result<()> {
    use crate::{constants::*, error::EndowmentError};
    require!(
        max_buy_per_tx > 0
            && max_buy_per_tx <= max_buy_per_day
            && (MIN_PRICE_IMPACT_BPS..=MAX_PRICE_IMPACT_BPS).contains(&max_price_impact_bps),
        EndowmentError::InvalidBuybackLimits
    );
    Ok(())
}

#[account]
#[derive(InitSpace)]
pub struct Landlord {
    pub owner: Pubkey,
    /// The owner's PUMP associated token account the endowment is delegated on.
    pub pump_account: Pubkey,
    /// PUMP the landlord keeps. Only the balance above this is swept.
    pub baseline: u64,
    pub total_contributed: u64,
    pub registered_at: i64,
    pub last_sweep_at: i64,
    pub bump: u8,
}

impl Landlord {
    /// How much of `balance` is sweepable, lowering the baseline first if the
    /// landlord has spent below it. Returns (amount, new_baseline).
    pub fn sweepable(&self, balance: u64, delegated: u64) -> (u64, u64) {
        let baseline = self.baseline.min(balance);
        let amount = (balance - baseline).min(delegated);
        (amount, baseline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landlord(baseline: u64) -> Landlord {
        Landlord {
            owner: Pubkey::default(),
            pump_account: Pubkey::default(),
            baseline,
            total_contributed: 0,
            registered_at: 0,
            last_sweep_at: 0,
            bump: 0,
        }
    }

    #[test]
    fn sweeps_only_above_baseline() {
        assert_eq!(landlord(100).sweepable(350, u64::MAX), (250, 100));
    }

    #[test]
    fn baseline_drops_when_landlord_spends_below_it() {
        assert_eq!(landlord(100).sweepable(40, u64::MAX), (0, 40));
    }

    #[test]
    fn capped_by_remaining_delegation() {
        assert_eq!(landlord(0).sweepable(500, 120), (120, 0));
    }
}
