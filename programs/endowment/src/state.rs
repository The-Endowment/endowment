use anchor_lang::prelude::*;

use crate::{constants::*, error::EndowmentError};

/// One endowment instance. Seeds: [CONFIG_SEED, coin_mint, creator].
#[account]
#[derive(InitSpace)]
pub struct Config {
    /// Whoever created the instance; part of its address.
    pub creator: Pubkey,
    /// Adjusts bounded parameters. Can be renounced for good.
    pub admin: Pubkey,
    /// Proposed next admin; must sign `accept_admin`. Default = none.
    pub pending_admin: Pubkey,
    /// Can pause cranks for at most MAX_PAUSE_SECONDS. Cannot move funds.
    pub guardian: Pubkey,
    /// The meme coin this endowment holds forever.
    pub coin_mint: Pubkey,
    /// The asset the coin pays its holders (e.g. PUMP), which the endowment spends.
    pub dividend_mint: Pubkey,
    /// Unix timestamp; cranks are blocked while `now < paused_until`.
    pub paused_until: i64,
    pub total_swept: u64,
    pub landlord_count: u32,
    pub bump: u8,
    pub authority_bump: u8,

    /// The Raydium CPMM coin/dividend pool buybacks trade against. Checked at creation.
    pub pool: Pubkey,
    /// Buyback limits, in dividend base units, bounded by `validate_limits`.
    pub max_buy_per_tx: u64,
    pub max_buy_per_day: u64,
    /// Slippage a single buyback may cause, beyond fees.
    pub max_price_impact_bps: u16,
    /// Rolling 24-hour buyback window.
    pub day_start: i64,
    pub bought_today: u64,
    pub total_dividend_spent: u64,
    pub total_coin_bought: u64,

    /// Buyback pacing and the crank tip.
    pub min_buy_interval_secs: i64,
    pub last_buy_at: i64,
    pub tip_bps: u16,
    pub total_tips: u64,

    /// Locked at creation: share of each buyback donated to the flagship endowment.
    pub donation_bps: u16,
    pub total_donated: u64,

    /// Landlord sweeps run only while `active`. See `CommitmentCount`.
    pub activate_bps: u16,
    pub deactivate_bps: u16,
    pub active: bool,
    pub count: CommitmentCount,

    /// One-way: once set, landlord sweeps are closed forever.
    pub contribution_cap: u64,
    pub closed: bool,
    /// After `closed`, the share of each buyback that buys the coin; the rest
    /// becomes permanently locked liquidity.
    pub buy_bps: u16,
    pub total_liquidity_dividend: u64,
    pub total_lp_tokens: u64,
}

/// A daily, permissionless count of how much of the coin the registered landlords hold.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, InitSpace, Debug, PartialEq)]
pub struct CommitmentCount {
    /// Increments each time a count starts; landlords remember the last round they were counted in.
    pub round: u64,
    pub open: bool,
    pub started_at: i64,
    /// Landlords that must be counted before the round can finish.
    pub expected: u32,
    pub counted: u32,
    pub committed: u64,
    /// Result of the last finished round.
    pub last_committed_bps: u16,
    pub last_finished_at: i64,
}

/// Everything a creator chooses for a new endowment. All bounded; the pool and
/// both mints are accounts, validated against each other.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct CreateParams {
    /// `Pubkey::default()` means the creator.
    pub admin: Pubkey,
    /// `Pubkey::default()` means the creator.
    pub guardian: Pubkey,
    pub max_buy_per_tx: u64,
    pub max_buy_per_day: u64,
    pub max_price_impact_bps: u16,
    /// In coin base units. Contributions close for good once the vault holds this.
    pub contribution_cap: u64,
    pub activate_bps: u16,
    pub deactivate_bps: u16,
    pub buy_bps: u16,
    pub min_buy_interval_secs: i64,
    pub tip_bps: u16,
    /// One of ALLOWED_DONATION_BPS. Locked forever.
    pub donation_bps: u16,
}

impl Config {
    pub fn is_paused(&self, now: i64) -> bool {
        now < self.paused_until
    }

    /// Hysteresis: on at or above `activate_bps`, off below `deactivate_bps`,
    /// unchanged in between.
    pub fn apply_committed_bps(&mut self, committed_bps: u16) {
        if committed_bps >= self.activate_bps {
            self.active = true;
        } else if committed_bps < self.deactivate_bps {
            self.active = false;
        }
    }

    /// Signer seeds for this instance's authority PDA.
    pub fn authority_seeds<'a>(config: &'a Pubkey, bump: &'a [u8; 1]) -> [&'a [u8]; 3] {
        [AUTHORITY_SEED, config.as_ref(), &bump[..]]
    }
}

pub fn validate_limits(max_buy_per_tx: u64, max_buy_per_day: u64, max_price_impact_bps: u16) -> Result<()> {
    require!(
        max_buy_per_tx > 0
            && max_buy_per_tx <= max_buy_per_day
            && (MIN_PRICE_IMPACT_BPS..=MAX_PRICE_IMPACT_BPS).contains(&max_price_impact_bps),
        EndowmentError::InvalidBuybackLimits
    );
    Ok(())
}

pub fn validate_activation(activate_bps: u16, deactivate_bps: u16) -> Result<()> {
    require!(
        activate_bps <= MAX_ACTIVATION_BPS && deactivate_bps <= activate_bps,
        EndowmentError::InvalidActivation
    );
    Ok(())
}

pub fn validate_buy_params(buy_bps: u16, min_buy_interval_secs: i64, tip_bps: u16) -> Result<()> {
    let (lo, hi) = MIN_BUY_INTERVAL_BOUNDS;
    require!(
        buy_bps <= 10_000 && (lo..=hi).contains(&min_buy_interval_secs) && tip_bps <= MAX_TIP_BPS,
        EndowmentError::InvalidBuyParams
    );
    Ok(())
}

pub fn validate_donation(donation_bps: u16, dividend_mint: &Pubkey, config: &Pubkey) -> Result<()> {
    require!(ALLOWED_DONATION_BPS.contains(&donation_bps), EndowmentError::InvalidDonation);
    if donation_bps > 0 {
        // Only the flagship's dividend asset can be donated, and the flagship
        // doesn't donate to itself.
        require_keys_eq!(*dividend_mint, FLAGSHIP_DIVIDEND_MINT, EndowmentError::InvalidDonation);
        require_keys_neq!(*config, FLAGSHIP_CONFIG, EndowmentError::InvalidDonation);
    }
    Ok(())
}

/// A landlord of one endowment. Seeds: [LANDLORD_SEED, config, owner].
#[account]
#[derive(InitSpace)]
pub struct Landlord {
    /// The endowment this landlord belongs to.
    pub config: Pubkey,
    pub owner: Pubkey,
    /// The owner's dividend associated token account the endowment is delegated on.
    pub dividend_account: Pubkey,
    /// Dividend the landlord keeps. Only the balance above this is swept.
    pub baseline: u64,
    pub total_contributed: u64,
    pub registered_at: i64,
    pub last_sweep_at: i64,
    pub bump: u8,
    /// The owner's coin associated token account, counted toward the activation threshold.
    pub coin_account: Pubkey,
    /// The last commitment round this landlord was counted in (or joined during).
    pub counted_round: u64,
    /// Coin counted for this landlord in `counted_round`.
    pub counted_balance: u64,
    /// The round that was open when this landlord registered (0 if none).
    pub joined_round: u64,
}

impl Landlord {
    /// Whether this landlord was actually counted in `round` (as opposed to
    /// having joined during it).
    pub fn counted_in(&self, round: u64) -> bool {
        round != 0 && self.counted_round == round && self.joined_round != round
    }

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
            config: Pubkey::default(),
            owner: Pubkey::default(),
            dividend_account: Pubkey::default(),
            baseline,
            total_contributed: 0,
            registered_at: 0,
            last_sweep_at: 0,
            bump: 0,
            coin_account: Pubkey::default(),
            counted_round: 0,
            counted_balance: 0,
            joined_round: 0,
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

    #[test]
    fn activation_has_hysteresis() {
        let mut config = Config::try_from_slice(&vec![0u8; Config::INIT_SPACE]).unwrap();
        config.activate_bps = 3_000;
        config.deactivate_bps = 2_500;
        config.apply_committed_bps(2_999);
        assert!(!config.active);
        config.apply_committed_bps(3_000);
        assert!(config.active);
        config.apply_committed_bps(2_600);
        assert!(config.active, "between the lines, state is unchanged");
        config.apply_committed_bps(2_499);
        assert!(!config.active);
        config.apply_committed_bps(2_900);
        assert!(!config.active, "between the lines, state is unchanged");
    }

    #[test]
    fn donation_is_one_of_the_fixed_rates_and_only_in_the_flagship_asset() {
        let other = Pubkey::new_unique();
        let instance = Pubkey::new_unique();
        for bps in ALLOWED_DONATION_BPS {
            assert!(validate_donation(bps, &FLAGSHIP_DIVIDEND_MINT, &instance).is_ok());
        }
        assert!(validate_donation(15, &FLAGSHIP_DIVIDEND_MINT, &instance).is_err());
        assert!(validate_donation(40, &FLAGSHIP_DIVIDEND_MINT, &instance).is_err());
        assert!(validate_donation(0, &other, &instance).is_ok());
        assert!(validate_donation(10, &other, &instance).is_err());
        // The flagship never donates to itself.
        assert!(validate_donation(10, &FLAGSHIP_DIVIDEND_MINT, &FLAGSHIP_CONFIG).is_err());
        assert!(validate_donation(0, &FLAGSHIP_DIVIDEND_MINT, &FLAGSHIP_CONFIG).is_ok());
    }
}
