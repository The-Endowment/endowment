use anchor_lang::prelude::*;

#[error_code]
pub enum EndowmentError {
    #[msg("Only the program's upgrade authority can initialize")]
    NotUpgradeAuthority,
    #[msg("The endowment is paused")]
    Paused,
    #[msg("The PUMP account is not delegated to the endowment")]
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
    #[msg("Nothing to buy: the PUMP vault is empty or today's cap is used up")]
    NothingToBuy,
    #[msg("Buyback exceeds the daily cap")]
    DailyCapReached,
    #[msg("Account does not belong to the endowment's pool")]
    WrongPool,
    #[msg("Pool account data is not a Raydium CPMM pool")]
    InvalidPoolData,
    #[msg("Fill is below the price-impact floor")]
    PriceImpactTooHigh,
    #[msg("Fill is below the caller's minimum")]
    SlippageExceeded,
    #[msg("Too soon since the last buyback")]
    BuyTooSoon,
    #[msg("Buy parameters are outside the hard-coded bounds")]
    InvalidBuyParams,
    #[msg("Activation thresholds are outside the hard-coded bounds")]
    InvalidActivation,
    #[msg("Landlord sweeps are not active: not enough $PENIS is committed")]
    NotActive,
    #[msg("Landlord contributions are closed")]
    ContributionsClosed,
    #[msg("The contribution cap has not been reached")]
    CapNotReached,
    #[msg("A commitment count is already open")]
    CountOpen,
    #[msg("No commitment count is open")]
    CountNotOpen,
    #[msg("A commitment count already ran in the last 24 hours")]
    CountTooSoon,
    #[msg("Not every landlord has been counted yet")]
    CountIncomplete,
    #[msg("Invalid landlord or $PENIS account in the count")]
    InvalidCountAccount,
    #[msg("The $PENIS vault would shrink")]
    VaultWouldShrink,
}
