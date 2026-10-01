use anchor_lang::prelude::*;

#[error_code]
pub enum EndowmentError {
    #[msg("The coin and dividend mints must differ")]
    SameMint,
    #[msg("The endowment is paused")]
    Paused,
    #[msg("The dividend account is not delegated to the endowment")]
    NotDelegated,
    #[msg("Only the guardian can do this")]
    NotGuardian,
    #[msg("Only the admin can do this")]
    NotAdmin,
    #[msg("Only the proposed admin can accept")]
    NotPendingAdmin,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Buyback limits are outside the hard-coded bounds")]
    InvalidBuybackLimits,
    #[msg("Nothing to buy: too little in the vault or in today's allowance")]
    NothingToBuy,
    #[msg("Account does not belong to the endowment's pool")]
    WrongPool,
    #[msg("Pool account data is not a Raydium CPMM pool")]
    InvalidPoolData,
    #[msg("Fill is below the price floor")]
    PriceImpactTooHigh,
    #[msg("Fill is below the caller's minimum")]
    SlippageExceeded,
    #[msg("Too soon since the last buyback")]
    BuyTooSoon,
    #[msg("Buy parameters are outside the hard-coded bounds")]
    InvalidBuyParams,
    #[msg("Activation thresholds are outside the hard-coded bounds")]
    InvalidActivation,
    #[msg("Landlord sweeps are not active: not enough of the coin is committed")]
    NotActive,
    #[msg("The endowment is retired: no more sweeps or registrations")]
    Retired,
    #[msg("A commitment count already started in the last 24 hours")]
    CountTooSoon,
    #[msg("Each landlord must be passed as its record, registered coin account and registered dividend account")]
    InvalidCountAccount,
    #[msg("The coin vault would shrink")]
    VaultWouldShrink,
    #[msg("Donation must be 0, 10, 20 or 30 bps, and only in the flagship's dividend asset")]
    InvalidDonation,
    #[msg("The contribution cap must be greater than zero")]
    InvalidContributionCap,
    #[msg("Not the flagship endowment's dividend vault")]
    WrongFlagshipVault,
    #[msg("A pause is active or ended too recently")]
    PauseCooldown,
    #[msg("No parameter change is pending")]
    NoPendingParams,
    #[msg("The parameter timelock has not elapsed")]
    TimelockNotElapsed,
    #[msg("The admin can only renounce with production activation thresholds")]
    RenounceThresholds,
    #[msg("A fee is above the hard-coded ceiling")]
    FeeTooHigh,
    #[msg("A mint's transfer hook is switched on")]
    TransferHookEnabled,
    #[msg("Not enough price history in the pool's observation account")]
    TwapUnavailable,
    #[msg("The coin's price is too far above its time-weighted average")]
    PriceAboveTwap,
    #[msg("The pool has swaps disabled")]
    PoolSwapDisabled,
    #[msg("A Raydium call changed something it must not")]
    CpiInvariant,
    #[msg("Landlords must hold at least the minimum stake")]
    StakeTooSmall,
    #[msg("A commitment count is already open")]
    CountOpen,
    #[msg("No commitment count is open")]
    NoOpenCount,
    #[msg("This landlord isn't part of the open count, or was already counted in it")]
    NotInCount,
    #[msg("Not every landlord has been counted and the count hasn't timed out")]
    CountIncomplete,
    #[msg("This landlord is still delegated and holds the minimum stake")]
    NotPrunable,
    #[msg("Parameters are outside the hard-coded bounds")]
    InvalidParams,
    #[msg("A vault is frozen, so buybacks can't run")]
    VaultFrozen,
    #[msg("No commitment count has finished recently")]
    CountStale,
    #[msg("This parameter proposal has expired")]
    ProposalExpired,
    #[msg("Only the admin can apply a proposal in its first day")]
    ApplyGrace,
    #[msg("Cancel pending changes before renouncing")]
    PendingChange,
    #[msg("The retirement timelock has not elapsed")]
    RetireNotReady,
    #[msg("This mint has an authority or extension the endowment can't accept")]
    UnsafeMint,
    #[msg("The coin's price is too far below its time-weighted average")]
    PriceBelowTwap,
    #[msg("The pool's quote for this buy is below the TWAP floor")]
    FloorAboveQuote,
    #[msg("The refresher hasn't read any landlord since the last count began")]
    NotAttested,
    #[msg("Set a refresher before renouncing")]
    NoRefresher,
    #[msg("Only the refresher can do this")]
    NotRefresher,
    #[msg("The endowment's direct coin vault has reached its funding goal")]
    Completed,
    #[msg("Legacy balance-based collection and enrollment are disabled")]
    LegacyCollectionDisabled,
    #[msg("Explicit version 4 collection consent is required")]
    UnsupportedCollectionVersion,
    #[msg("Only the enabled reporter may collect rewards")]
    NotReporter,
    #[msg("Report does not match the current consent, policy or collection epoch")]
    StaleReport,
    #[msg("Report nonce must be the next unused nonce")]
    ReportReplay,
    #[msg("Report has expired, is from the future, or exceeds its maximum lifetime")]
    ReportExpired,
    #[msg("The source token balance changed since the report")]
    SourceBalanceChanged,
    #[msg("Report amount exceeds a collection limit or available balance")]
    CollectionLimit,
    #[msg("Reporter address must be nonzero")]
    InvalidReporter,
    #[msg("Reporter policy is frozen after admin renunciation")]
    ReporterFrozen,
}
