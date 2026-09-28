use anchor_lang::prelude::*;

#[error_code]
pub enum EndowmentError {
    #[msg("Only the program's upgrade authority can initialize")]
    NotUpgradeAuthority,
    #[msg("Supply target is outside the hard-coded bounds")]
    SupplyTargetOutOfBounds,
    #[msg("The endowment is paused")]
    Paused,
    #[msg("The PUMP account is not delegated to the endowment")]
    NotDelegated,
    #[msg("Only the guardian can do this")]
    NotGuardian,
    #[msg("Arithmetic overflow")]
    Overflow,
}
