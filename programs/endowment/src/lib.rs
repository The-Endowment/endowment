pub mod constants;
pub mod error;
pub mod events;
pub mod instructions;
pub mod math;
pub mod raydium;
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
        pool: Pubkey,
        max_buy_per_tx: u64,
        max_buy_per_day: u64,
        max_price_impact_bps: u16,
    ) -> Result<()> {
        instructions::initialize::handle_initialize(
            ctx,
            admin,
            guardian,
            supply_target_bps,
            pool,
            max_buy_per_tx,
            max_buy_per_day,
            max_price_impact_bps,
        )
    }

    pub fn buyback(ctx: Context<Buyback>, amount_in: u64, min_out: u64) -> Result<()> {
        instructions::buyback::handle_buyback(ctx, amount_in, min_out)
    }

    pub fn set_buyback_limits(
        ctx: Context<AdminOnly>,
        max_buy_per_tx: u64,
        max_buy_per_day: u64,
        max_price_impact_bps: u16,
    ) -> Result<()> {
        instructions::roles::handle_set_buyback_limits(ctx, max_buy_per_tx, max_buy_per_day, max_price_impact_bps)
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

    pub fn pause(ctx: Context<Pause>) -> Result<()> {
        instructions::pause::handle_pause(ctx)
    }

    pub fn unpause(ctx: Context<Unpause>) -> Result<()> {
        instructions::pause::handle_unpause(ctx)
    }

    pub fn set_guardian(ctx: Context<AdminOnly>, new_guardian: Pubkey) -> Result<()> {
        instructions::roles::handle_set_guardian(ctx, new_guardian)
    }

    pub fn propose_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        instructions::roles::handle_propose_admin(ctx, new_admin)
    }

    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        instructions::roles::handle_accept_admin(ctx)
    }
}
