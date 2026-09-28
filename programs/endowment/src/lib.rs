pub mod constants;
pub mod error;
pub mod events;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("5VBiPX39xFTgwRaUbC3F3HCuVcM3VkTuYDkxwrhYby2u");

/// The $PENIS Endowment.
///
/// There is deliberately no instruction that transfers tokens out of the
/// $PENIS vault. Landlords can always leave by revoking their token
/// delegation directly with the token program.
#[program]
pub mod endowment {
    use super::*;

    pub fn initialize(
        ctx: Context<Initialize>,
        admin: Pubkey,
        guardian: Pubkey,
        supply_target_bps: u16,
    ) -> Result<()> {
        instructions::initialize::handle_initialize(ctx, admin, guardian, supply_target_bps)
    }

    pub fn register_landlord(ctx: Context<RegisterLandlord>) -> Result<()> {
        instructions::register_landlord::handle_register_landlord(ctx)
    }

    pub fn deregister_landlord(ctx: Context<DeregisterLandlord>) -> Result<()> {
        instructions::deregister_landlord::handle_deregister_landlord(ctx)
    }

    pub fn sweep(ctx: Context<Sweep>) -> Result<()> {
        instructions::sweep::handle_sweep(ctx)
    }

    pub fn pause(ctx: Context<SetPause>) -> Result<()> {
        instructions::pause::handle_pause(ctx)
    }

    pub fn unpause(ctx: Context<SetPause>) -> Result<()> {
        instructions::pause::handle_unpause(ctx)
    }
}
