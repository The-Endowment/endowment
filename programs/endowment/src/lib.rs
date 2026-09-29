pub mod constants;
pub mod error;
pub mod events;
pub mod instructions;
pub mod math;
pub mod raydium;
pub mod state;
pub mod transfer;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("5VBiPX39xFTgwRaUbC3F3HCuVcM3VkTuYDkxwrhYby2u");

/// Endowments for dividend-paying meme coins.
///
/// One shared contract hosts any number of endowments, one per (coin, creator).
/// Each endowment collects the dividend its landlords delegate and spends it
/// buying its coin, which it holds forever.
///
/// There is deliberately no instruction that transfers tokens out of any
/// endowment's coin vault or liquidity (LP) vault. Landlords can always leave
/// by revoking their token delegation directly with the token program, and by
/// deregistering, neither of which a pause can block.
/// Endowments are isolated from each other: every account an instruction
/// touches is derived from, or checked against, that endowment's config.
#[program]
pub mod endowment {
    use super::*;

    /// Permissionless: creates an endowment for a coin and its dividend asset.
    pub fn create_endowment(ctx: Context<CreateEndowment>, params: CreateParams) -> Result<()> {
        instructions::create_endowment::handle_create_endowment(ctx, params)
    }

    /// Landlords.
    pub fn register_landlord(ctx: Context<RegisterLandlord>) -> Result<()> {
        instructions::register_landlord::handle_register_landlord(ctx)
    }

    pub fn resync_baseline(ctx: Context<ResyncBaseline>) -> Result<()> {
        instructions::resync_baseline::handle_resync_baseline(ctx)
    }

    pub fn deregister_landlord(ctx: Context<DeregisterLandlord>) -> Result<()> {
        instructions::deregister_landlord::handle_deregister_landlord(ctx)
    }

    /// Permissionless cranks.
    pub fn prune_landlord(ctx: Context<PruneLandlord>) -> Result<()> {
        instructions::prune_landlord::handle_prune_landlord(ctx)
    }

    pub fn sweep(ctx: Context<Sweep>) -> Result<()> {
        instructions::sweep::handle_sweep(ctx)
    }

    pub fn buyback<'info>(ctx: Context<'info, Buyback<'info>>, min_out: u64) -> Result<()> {
        instructions::buyback::handle_buyback(ctx, min_out)
    }

    /// The daily commitment count: begin, count landlords in batches, finish.
    pub fn begin_count(ctx: Context<BeginCount>) -> Result<()> {
        instructions::count::handle_begin_count(ctx)
    }

    pub fn count_landlords<'info>(ctx: Context<'info, CountLandlords<'info>>) -> Result<()> {
        instructions::count::handle_count_landlords(ctx)
    }

    pub fn finish_count(ctx: Context<FinishCount>) -> Result<()> {
        instructions::count::handle_finish_count(ctx)
    }

    /// Decrease-only re-read of landlords' balances between counts.
    pub fn refresh_landlords<'info>(ctx: Context<'info, RefreshLandlords<'info>>) -> Result<()> {
        instructions::count::handle_refresh_landlords(ctx)
    }

    /// Parameters: proposed by the admin, applied by anyone after 72 hours.
    pub fn propose_params(ctx: Context<AdminOnly>, params: Params) -> Result<()> {
        instructions::roles::handle_propose_params(ctx, params)
    }

    pub fn cancel_params(ctx: Context<AdminOnly>) -> Result<()> {
        instructions::roles::handle_cancel_params(ctx)
    }

    pub fn apply_params(ctx: Context<ApplyParams>) -> Result<()> {
        instructions::roles::handle_apply_params(ctx)
    }

    /// Guardian and admin, per endowment.
    pub fn pause(ctx: Context<Pause>) -> Result<()> {
        instructions::pause::handle_pause(ctx)
    }

    pub fn unpause(ctx: Context<Unpause>) -> Result<()> {
        instructions::pause::handle_unpause(ctx)
    }

    pub fn retire(ctx: Context<AdminOnly>) -> Result<()> {
        instructions::roles::handle_retire(ctx)
    }

    pub fn renounce_admin(ctx: Context<AdminOnly>) -> Result<()> {
        instructions::roles::handle_renounce_admin(ctx)
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
