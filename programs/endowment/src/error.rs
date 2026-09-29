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
    #[msg("A commitment count already ran in the last 24 hours")]
    CountTooSoon,
    #[msg("The count must list every landlord's coin and dividend account, in roster order")]
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
    #[msg("The roster is full and no smaller landlord was given to replace")]
    RosterFull,
    #[msg("Not the landlord with the smallest recorded stake, or not smaller than yours")]
    InvalidEviction,
    #[msg("This landlord is still delegated and holds the minimum stake")]
    NotPrunable,
    #[msg("Parameters are outside the hard-coded bounds")]
    InvalidParams,
}
