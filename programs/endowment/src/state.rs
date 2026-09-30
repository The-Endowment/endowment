use anchor_lang::prelude::*;

use crate::{constants::*, error::EndowmentError};

/// Every tunable parameter. Set at creation; afterwards changed only through
/// `propose_params` → 72h → `apply_params`, and frozen for good by `renounce_admin`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, InitSpace, PartialEq)]
pub struct Params {
    /// Buyback limits, in dividend base units.
    pub max_buy_per_tx: u64,
    /// The buy allowance refills at this much per 24 hours.
    pub max_buy_per_day: u64,
    /// Slippage a single buyback may cause beyond fees, against the TWAP.
    pub max_price_impact_bps: u16,
    /// How far the spot price may sit from the TWAP, either way, for a buyback
    /// to run; also the floor's allowance for that drift. See
    /// MIN/MAX_TWAP_DEVIATION_BPS.
    pub max_twap_deviation_bps: u16,
    /// Buys smaller than this are skipped (not worth a transaction).
    pub min_buy_amount: u64,
    pub min_buy_interval_secs: i64,
    pub tip_bps: u16,
    /// After the milestone, the share of each buyback that buys the coin; the
    /// rest becomes permanently locked liquidity.
    pub buy_bps: u16,
    /// Landlord sweeps switch on at `activate_bps` of supply committed, off below `deactivate_bps`.
    pub activate_bps: u16,
    pub deactivate_bps: u16,
    /// Minimum share of supply a landlord must hold to register and be counted.
    pub min_stake_bps: u16,
    /// The key whose reads make landlords eligible to count (see
    /// `refresh_landlords`). It can't move funds, and each read can only lower
    /// a landlord's recorded balance. It is trusted, though: it chooses when
    /// it reads, so a refresher that colludes with landlords can time its reads
    /// to when one holding sits in each of several wallets and have it counted
    /// more than once. `REQUIRED_ATTESTATIONS` spaced reads per landlord per
    /// round make that a race to win several times over, and every read is
    /// public (`LandlordsAttested` lists the landlords). `Pubkey::default()` =
    /// none, so nobody counts and sweeps stay off.
    pub refresher: Pubkey,
}

impl Params {
    /// Every hard-coded bound. `donation_bps` is locked at creation and checked with it.
    pub fn validate(&self, donation_bps: u16) -> Result<()> {
        let (lo, hi) = MIN_BUY_INTERVAL_BOUNDS;
        require!(
            self.max_buy_per_tx > 0
                && self.max_buy_per_tx <= self.max_buy_per_day
                && (MIN_PRICE_IMPACT_BPS..=MAX_PRICE_IMPACT_BPS).contains(&self.max_price_impact_bps)
                && (MIN_TWAP_DEVIATION_BPS..=MAX_TWAP_DEVIATION_BPS).contains(&self.max_twap_deviation_bps),
            EndowmentError::InvalidBuybackLimits
        );
        require!(
            self.min_buy_amount <= self.max_buy_per_tx
                && (lo..=hi).contains(&self.min_buy_interval_secs)
                && self.tip_bps <= MAX_TIP_BPS
                && self.buy_bps <= 10_000
                && self.tip_bps + donation_bps <= MAX_TIP_PLUS_DONATION_BPS,
            EndowmentError::InvalidBuyParams
        );
        require!(
            self.activate_bps <= MAX_ACTIVATION_BPS
                && self.deactivate_bps <= self.activate_bps
                && (self.activate_bps == 0 || self.deactivate_bps >= MIN_DEACTIVATE_BPS),
            EndowmentError::InvalidActivation
        );
        require!(self.min_stake_bps <= MAX_MIN_STAKE_BPS, EndowmentError::InvalidParams);
        Ok(())
    }
}

/// One daily commitment count, run across as many transactions as needed.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, InitSpace, PartialEq)]
pub struct CountRound {
    /// Increments at each `begin_count`. 0 = no count has ever started.
    pub round: u64,
    pub open: bool,
    pub started_at: i64,
    /// The coin's supply when the round began; the denominator.
    pub supply: u64,
    /// Landlords registered before the round began: each must be counted (or
    /// the round must time out) before it can finish.
    pub expected: u32,
    pub counted: u32,
    /// Sum of what the counted landlords count for.
    pub committed: u64,
    /// The minimum stake for this round, fixed when it began, so a parameter
    /// change mid-round can't treat landlords in one round differently.
    pub min_stake: u64,
}

/// A parameter change waiting out the timelock.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, InitSpace, PartialEq)]
pub struct PendingParams {
    pub params: Params,
    /// 0 = nothing pending.
    pub effective_at: i64,
}

/// One endowment instance. Seeds: [CONFIG_SEED, coin_mint, creator].
#[account]
#[derive(InitSpace)]
pub struct Config {
    pub version: u8,
    /// Whoever created the instance; part of its address.
    pub creator: Pubkey,
    /// Proposes bounded, timelocked parameter changes. Can be renounced for good.
    pub admin: Pubkey,
    /// Proposed next admin; must sign `accept_admin`. Default = none.
    pub pending_admin: Pubkey,
    /// Can pause for at most MAX_PAUSE_SECONDS, then must wait out a cooldown.
    /// Cleared when the admin renounces. Cannot move funds.
    pub guardian: Pubkey,
    /// The meme coin this endowment holds forever.
    pub coin_mint: Pubkey,
    /// The asset the coin pays its holders (e.g. PUMP), which the endowment spends.
    pub dividend_mint: Pubkey,
    /// The Raydium CPMM coin/dividend pool buybacks trade against. Checked at creation.
    pub pool: Pubkey,
    pub bump: u8,
    pub authority_bump: u8,

    pub params: Params,
    pub pending: PendingParams,
    /// Locked at creation: share of each buyback donated to the flagship endowment.
    pub donation_bps: u16,
    /// In coin base units. Once the endowment has bought this much, buybacks
    /// switch to the buy/liquidity split. Contributions keep flowing.
    pub contribution_cap: u64,

    /// Unix timestamp; everything but leaving is blocked while `now < paused_until`.
    pub paused_until: i64,
    /// One-way: no more sweeps or registrations. Set only by the admin, after
    /// the parameter timelock (`retire_at`).
    pub retired: bool,
    /// When a proposed retirement can take effect; 0 = none proposed.
    pub retire_at: i64,
    /// One-way: set when `total_coin_bought` reaches `contribution_cap`.
    pub milestone_reached: bool,

    /// Landlord sweeps run only while `active`. See the daily count
    /// (`begin_count` → `count_landlords` → `finish_count`).
    pub active: bool,
    /// When the last count finished, and what it found.
    pub last_count_at: i64,
    pub last_count_bps: u16,
    pub last_committed: u64,
    /// Last refresh by the refresher, and last sweep, for monitoring.
    pub last_attested_at: i64,
    pub last_sweep_at: i64,

    /// Registered landlords (no limit).
    pub landlord_count: u32,
    /// The current (or last) count round and its running tally.
    pub count: CountRound,

    /// Buy pacing: a token bucket holding at most `max_buy_per_tx`, refilling at
    /// `max_buy_per_day` per 24 hours.
    pub buy_allowance: u64,
    pub allowance_updated_at: i64,
    pub last_buy_at: i64,

    pub total_swept: u64,
    pub total_dividend_spent: u64,
    /// Coin received from swaps (including coin later deposited as liquidity).
    pub total_coin_bought: u64,
    /// Coin kept in the vault (net of liquidity deposits).
    pub total_coin_retained: u64,
    pub total_liquidity_dividend: u64,
    pub total_liquidity_coin: u64,
    pub total_lp_tokens: u64,
    pub total_tips: u64,
    pub total_donated: u64,

    /// Bumped whenever the refresher changes (resigns, or a new one is applied).
    /// Reads made under an earlier epoch don't count (FC-R3-03).
    pub refresher_epoch: u32,

    /// Room for future fields without a migration.
    pub reserved: [u8; 124],
}

/// Everything a creator chooses for a new endowment. All bounded; the pool and
/// both mints are accounts, validated against each other.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct CreateParams {
    /// `Pubkey::default()` means the creator.
    pub admin: Pubkey,
    /// `Pubkey::default()` means the creator.
    pub guardian: Pubkey,
    pub params: Params,
    /// In coin base units. See `Config::contribution_cap`.
    pub contribution_cap: u64,
    /// One of ALLOWED_DONATION_BPS. Locked forever.
    pub donation_bps: u16,
}

impl Config {
    pub fn is_paused(&self, now: i64) -> bool {
        now < self.paused_until
    }

    /// Sweeps run only while active, and (outside a 0-threshold test window)
    /// only while a count has finished recently: a count nobody runs can't keep
    /// an endowment switched on.
    pub fn sweeps_on(&self, now: i64) -> bool {
        self.active
            && (self.params.activate_bps == 0 || now.saturating_sub(self.last_count_at) <= ACTIVE_MAX_AGE_SECS)
    }

    /// When the open round's timeout clock started: its start, or the end of a
    /// pause that overlapped it, so a pause can't run the clock out.
    pub fn count_clock_start(&self) -> i64 {
        if self.paused_until > self.count.started_at {
            self.paused_until
        } else {
            self.count.started_at
        }
    }

    /// Hysteresis: on at or above `activate_bps`, off below `deactivate_bps`,
    /// unchanged in between.
    pub fn apply_committed_bps(&mut self, committed_bps: u16) {
        if committed_bps >= self.params.activate_bps {
            self.active = true;
        } else if committed_bps < self.params.deactivate_bps {
            self.active = false;
        }
    }

    /// Signer seeds for this instance's authority PDA.
    pub fn authority_seeds<'a>(config: &'a Pubkey, bump: &'a [u8; 1]) -> [&'a [u8]; 3] {
        [AUTHORITY_SEED, config.as_ref(), &bump[..]]
    }

    /// The most a dividend vault may hold (sweeps and donations stop there):
    /// MAX_VAULT_DAYS_OF_BUYS days of what buybacks can actually spend, which
    /// is the smaller of the daily allowance and what the per-buy cap and the
    /// buy interval allow in a day (FC-R3-04).
    pub fn vault_cap(&self) -> u64 {
        let p = &self.params;
        let interval = p.min_buy_interval_secs.max(1) as u128;
        let by_interval = (p.max_buy_per_tx as u128 * 86_400 / interval).min(u64::MAX as u128) as u64;
        p.max_buy_per_day.min(by_interval).saturating_mul(MAX_VAULT_DAYS_OF_BUYS)
    }

    /// A landlord's refresher reads, if they were made under the current refresher.
    pub fn attestations_of(&self, landlord: &Landlord) -> u8 {
        if landlord.attestation_epoch == self.refresher_epoch {
            landlord.attestations
        } else {
            0
        }
    }

    /// The refresher resigns or changes: reads made under it stop counting, an open round
    /// (whose tally rests on those reads) closes without a result, and sweeps
    /// switch off until a count under a new refresher switches them back on
    /// (FC-R3-03).
    pub fn retire_refresher_reads(&mut self) {
        self.refresher_epoch = self.refresher_epoch.wrapping_add(1);
        self.count.open = false;
        self.active = false;
        // A later parameter change must not reactivate sweeps from an old result.
        self.last_count_bps = 0;
        self.last_count_at = 0;
        self.last_committed = 0;
    }

    /// The minimum coin a landlord must hold, given the coin's current supply.
    pub fn min_stake(&self, supply: u64) -> u64 {
        (supply as u128 * self.params.min_stake_bps as u128).div_ceil(10_000) as u64
    }

    /// Whether `landlord` belongs to the round now open and hasn't been counted in it.
    pub fn expects(&self, landlord: &Landlord) -> bool {
        self.count.open && landlord.joined_round < self.count.round && landlord.counted_round < self.count.round
    }

    /// Takes a departing landlord out of the open round, so leaving mid-count
    /// can't strand the round or let its coin be counted twice.
    pub fn remove_from_round(&mut self, landlord: &Landlord) {
        if !self.count.open || landlord.joined_round >= self.count.round {
            return;
        }
        self.count.expected = self.count.expected.saturating_sub(1);
        if landlord.counted_round == self.count.round {
            self.count.counted = self.count.counted.saturating_sub(1);
            self.count.committed = self.count.committed.saturating_sub(landlord.counted_amount);
        }
    }
}

pub fn validate_donation(donation_bps: u16, dividend_mint: &Pubkey, config: &Pubkey) -> Result<()> {
    require!(ALLOWED_DONATION_BPS.contains(&donation_bps), EndowmentError::InvalidDonation);
    if donation_bps > 0 {
        // Only once the flagship's address is fixed, only in the flagship's
        // dividend asset, and the flagship never donates to itself.
        require!(flagship_is_set(), EndowmentError::InvalidDonation);
        require_keys_eq!(*dividend_mint, FLAGSHIP_DIVIDEND_MINT, EndowmentError::InvalidDonation);
        require_keys_neq!(*config, flagship_config(), EndowmentError::InvalidDonation);
    }
    Ok(())
}

/// A landlord of one endowment. Seeds: [LANDLORD_SEED, config, owner].
#[account]
#[derive(InitSpace)]
pub struct Landlord {
    pub version: u8,
    /// The endowment this landlord belongs to.
    pub config: Pubkey,
    pub owner: Pubkey,
    /// The owner's dividend associated token account the endowment is delegated on.
    pub dividend_account: Pubkey,
    /// The owner's coin associated token account, counted toward activation.
    pub coin_account: Pubkey,
    /// Dividend the landlord keeps. Only the balance above this is swept. Only
    /// the owner can change it (`resync_baseline`).
    pub baseline: u64,
    pub total_contributed: u64,
    pub registered_at: i64,
    pub last_sweep_at: i64,
    pub bump: u8,

    /// The count round that was open (or last finished) when this landlord
    /// registered. It is counted from the next round on.
    pub joined_round: u64,
    /// The last round this landlord was counted in, and what it counted for.
    pub counted_round: u64,
    pub counted_amount: u64,
    /// Coin balance read at this landlord's last count, lowered by any later
    /// refresh that found less. The next count credits at most this, so coin
    /// must be held from one count to the next to count.
    pub snapshot: u64,
    /// True once a count (or refresh) has read this landlord while delegated.
    /// Cleared whenever it's found not delegated, so re-approving costs a round.
    pub snapshot_valid: bool,
    /// Reads by the endowment's refresher since this landlord's last count read,
    /// each at least MIN_ATTEST_SPACING_SECS after the one before. It counts
    /// only once this reaches REQUIRED_ATTESTATIONS: the reads land at times
    /// the landlord doesn't choose, each lowers `snapshot` to what it finds,
    /// and a delegation held only around count time is caught. Reset by a
    /// count read and whenever the landlord is found not delegated.
    pub attestations: u8,
    /// When the last of those reads was.
    pub last_attested_at: i64,
    /// The endowment's `refresher_epoch` when those reads were made. Reads from
    /// an earlier epoch (a refresher since resigned or replaced) don't count.
    pub attestation_epoch: u32,

    /// Room for future fields without a migration.
    pub reserved: [u8; 52],
}

impl Landlord {
    /// How much of `balance` is sweepable: only what sits above the baseline,
    /// capped by the remaining delegation. The baseline never moves here.
    pub fn sweepable(&self, balance: u64, delegated: u64) -> u64 {
        balance.saturating_sub(self.baseline).min(delegated)
    }

    /// What this landlord's coin balance now is worth, before the delegation,
    /// attestation and minimum-stake checks: the smaller of that and its
    /// recorded balance (zero at its first count, or after it was found not
    /// delegated).
    pub fn held(&self, balance: u64) -> u64 {
        if self.snapshot_valid {
            balance.min(self.snapshot)
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landlord(baseline: u64) -> Landlord {
        Landlord {
            version: LANDLORD_VERSION,
            config: Pubkey::default(),
            owner: Pubkey::default(),
            dividend_account: Pubkey::default(),
            coin_account: Pubkey::default(),
            baseline,
            total_contributed: 0,
            registered_at: 0,
            last_sweep_at: 0,
            bump: 0,
            joined_round: 0,
            counted_round: 0,
            counted_amount: 0,
            snapshot: 0,
            snapshot_valid: false,
            attestations: 0,
            last_attested_at: 0,
            attestation_epoch: 0,
            reserved: [0; 52],
        }
    }

    fn valid_params() -> Params {
        Params {
            max_buy_per_tx: 100,
            max_buy_per_day: 1_000,
            max_price_impact_bps: 100,
            max_twap_deviation_bps: 500,
            min_buy_amount: 10,
            min_buy_interval_secs: 600,
            tip_bps: 25,
            buy_bps: 10_000,
            activate_bps: 3_000,
            deactivate_bps: 2_500,
            min_stake_bps: 10,
            refresher: Pubkey::new_unique(),
        }
    }

    #[test]
    fn sweeps_only_above_baseline() {
        assert_eq!(landlord(100).sweepable(350, u64::MAX), 250);
    }

    #[test]
    fn a_dip_below_baseline_sweeps_nothing() {
        assert_eq!(landlord(100).sweepable(40, u64::MAX), 0);
    }

    #[test]
    fn capped_by_remaining_delegation() {
        assert_eq!(landlord(0).sweepable(500, 120), 120);
    }

    #[test]
    fn activation_has_hysteresis() {
        let mut config = Config::try_from_slice(&vec![0u8; Config::INIT_SPACE]).unwrap();
        config.params.activate_bps = 3_000;
        config.params.deactivate_bps = 2_500;
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
    fn params_bounds() {
        assert!(valid_params().validate(0).is_ok());
        let with = |f: &dyn Fn(&mut Params)| {
            let mut p = valid_params();
            f(&mut p);
            p
        };
        assert!(with(&|p| p.max_buy_per_tx = 0).validate(0).is_err());
        assert!(with(&|p| p.max_buy_per_tx = 1_001).validate(0).is_err());
        assert!(with(&|p| p.max_price_impact_bps = 301).validate(0).is_err());
        assert!(with(&|p| p.min_buy_amount = 101).validate(0).is_err());
        assert!(with(&|p| p.min_buy_interval_secs = 59).validate(0).is_err());
        assert!(with(&|p| p.tip_bps = 51).validate(0).is_err());
        assert!(with(&|p| p.tip_bps = 51).validate(0).is_err());
        assert!(with(&|p| p.tip_bps = 50).validate(30).is_ok());
        assert!(with(&|p| p.tip_bps = 50).validate(40).is_err());
        assert!(with(&|p| p.buy_bps = 10_001).validate(0).is_err());
        assert!(with(&|p| p.activate_bps = 5_001).validate(0).is_err());
        assert!(with(&|p| p.deactivate_bps = 3_001).validate(0).is_err());
        assert!(with(&|p| p.min_stake_bps = 501).validate(0).is_err());
        assert!(with(&|p| p.max_twap_deviation_bps = 99).validate(0).is_err());
        assert!(with(&|p| p.max_twap_deviation_bps = 100).validate(0).is_ok());
        assert!(with(&|p| p.max_twap_deviation_bps = 1_000).validate(0).is_ok());
        assert!(with(&|p| p.max_twap_deviation_bps = 1_001).validate(0).is_err());
        // R3-RF-04: with an activation line, a count that finds nothing always switches off.
        assert!(with(&|p| p.deactivate_bps = 0).validate(0).is_err());
        assert!(with(&|p| p.deactivate_bps = 1).validate(0).is_ok());
        assert!(with(&|p| {
            p.activate_bps = 0;
            p.deactivate_bps = 0
        })
        .validate(0)
        .is_ok());
    }

    #[test]
    fn min_stake_rounds_up() {
        let mut config = Config::try_from_slice(&vec![0u8; Config::INIT_SPACE]).unwrap();
        config.params.min_stake_bps = 10;
        assert_eq!(config.min_stake(1_000_000), 1_000);
        assert_eq!(config.min_stake(1_000_001), 1_001);
        config.params.min_stake_bps = 0;
        assert_eq!(config.min_stake(1_000_000), 0);
    }

    #[test]
    fn a_landlord_counts_only_what_it_held_since_its_last_count() {
        let mut l = landlord(0);
        assert_eq!(l.held(500), 0, "the first count only records the balance");
        l.snapshot_valid = true;
        l.snapshot = 300;
        assert_eq!(l.held(500), 300, "growth waits for the next count");
        assert_eq!(l.held(120), 120, "a drop counts at once");
    }

    #[test]
    fn leaving_mid_round_takes_the_landlord_out_of_the_tally() {
        let mut config = Config::try_from_slice(&vec![0u8; Config::INIT_SPACE]).unwrap();
        config.count = CountRound { round: 5, open: true, expected: 3, counted: 2, committed: 700, ..Default::default() };
        let mut counted = landlord(0);
        counted.joined_round = 4;
        counted.counted_round = 5;
        counted.counted_amount = 400;
        config.remove_from_round(&counted);
        assert_eq!((config.count.expected, config.count.counted, config.count.committed), (2, 1, 300));

        let mut waiting = landlord(0);
        waiting.joined_round = 1;
        waiting.counted_round = 4;
        assert!(config.expects(&waiting));
        config.remove_from_round(&waiting);
        assert_eq!((config.count.expected, config.count.counted, config.count.committed), (1, 1, 300));

        // Joined during this round: never part of it.
        let mut newcomer = landlord(0);
        newcomer.joined_round = 5;
        assert!(!config.expects(&newcomer));
        config.remove_from_round(&newcomer);
        assert_eq!(config.count.expected, 1);
    }

    #[test]
    fn the_flagship_config_is_the_pda_of_its_coin_and_creator() {
        let (expected, _) = Pubkey::find_program_address(
            &[b"config", FLAGSHIP_COIN_MINT.as_ref(), FLAGSHIP_CREATOR.as_ref()],
            &crate::ID,
        );
        assert_eq!(flagship_config(), expected);
    }

    #[cfg(not(feature = "test-flagship"))]
    #[test]
    fn no_donation_while_the_flagship_creator_is_a_placeholder() {
        assert!(!flagship_is_set());
        let instance = Pubkey::new_unique();
        assert!(validate_donation(0, &FLAGSHIP_DIVIDEND_MINT, &instance).is_ok());
        for bps in [10, 20, 30] {
            assert!(validate_donation(bps, &FLAGSHIP_DIVIDEND_MINT, &instance).is_err());
        }
    }

    #[cfg(feature = "test-flagship")]
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
        assert!(validate_donation(10, &FLAGSHIP_DIVIDEND_MINT, &flagship_config()).is_err());
        assert!(validate_donation(0, &FLAGSHIP_DIVIDEND_MINT, &flagship_config()).is_ok());
    }

    #[test]
    fn a_stale_count_switches_sweeps_off() {
        let mut config = Config::try_from_slice(&vec![0u8; Config::INIT_SPACE]).unwrap();
        config.active = true;
        config.params.activate_bps = 3_000;
        config.last_count_at = 1_000;
        assert!(config.sweeps_on(1_000 + ACTIVE_MAX_AGE_SECS));
        assert!(!config.sweeps_on(1_001 + ACTIVE_MAX_AGE_SECS));
        config.params.activate_bps = 0;
        assert!(config.sweeps_on(1_001 + ACTIVE_MAX_AGE_SECS), "not in a 0-threshold test window");
    }

    #[test]
    fn a_pause_restarts_the_count_timeout_clock() {
        let mut config = Config::try_from_slice(&vec![0u8; Config::INIT_SPACE]).unwrap();
        config.count.started_at = 100;
        assert_eq!(config.count_clock_start(), 100);
        config.paused_until = 5_000;
        assert_eq!(config.count_clock_start(), 5_000);
    }
}
