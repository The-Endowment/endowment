pub mod collection;
pub mod constants;
pub mod error;
pub mod events;
pub mod health;
pub mod instructions;
pub mod math;
pub mod raydium;
pub mod state;
pub mod transfer;

use anchor_lang::prelude::*;

pub use collection::*;
pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("5VBiPX39xFTgwRaUbC3F3HCuVcM3VkTuYDkxwrhYby2u");

/// The $PENIS Endowment.
///
/// Holder collections enter refundable custody before approved amounts can
/// buy its coin ($PENIS), which it holds forever. Only FLAGSHIP_CREATOR
/// can create it; the code is open source for any other project to deploy as
/// its own copy.
///
/// There is deliberately no instruction that transfers tokens out of the
/// endowment's coin vault or liquidity (LP) vault. Landlords can always leave
/// by revoking their token delegation directly with the token program, and by
/// deregistering, neither of which a pause can block. Every account an
/// instruction touches is derived from, or checked against, the endowment's
/// config.
#[program]
pub mod endowment {
    use super::*;

    /// Creates the endowment. Only FLAGSHIP_CREATOR can call it.
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

    /// Collector-signed collection into refundable custody. The legacy
    /// no-argument sweep ABI is deliberately invalid; it cannot bypass a hold.
    pub fn sweep(ctx: Context<Sweep>, nonce: u64, report: CollectionReport) -> Result<()> {
        instructions::sweep::handle_sweep(ctx, nonce, report)
    }

    pub fn initialize_collection(ctx: Context<InitializeCollection>, collector: Pubkey, reviewer: Pubkey) -> Result<()> {
        collection::policy::initialize(ctx, collector, reviewer)
    }
    pub fn enable_collection(ctx: Context<EnableCollection>) -> Result<()> {
        collection::consent::enable(ctx)
    }
    pub fn disable_collection(ctx: Context<DisableCollection>) -> Result<()> {
        collection::consent::disable(ctx)
    }
    pub fn review_collection(ctx: Context<ReviewCollection>, approved_amount: u64, evidence_hash: [u8; 32]) -> Result<()> {
        collection::review::review(ctx, approved_amount, evidence_hash)
    }
    pub fn release_collection(ctx: Context<SettleCollection>) -> Result<()> {
        collection::settlement::settle(ctx, true)
    }
    pub fn refund_collection(ctx: Context<SettleCollection>) -> Result<()> {
        collection::settlement::settle(ctx, false)
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

    /// The refresher posts the coin's public cumulative reward total; landlords'
    /// sweep allowances grow by what their counted coin earned (see `rewards`).
    pub fn post_reward_total(ctx: Context<PostRewardTotal>, total: u64) -> Result<()> {
        instructions::rewards::handle_post_reward_total(ctx, total)
    }

    /// Decrease-only re-read of landlords' balances between counts. When the
    /// endowment's refresher signs, it also attests them (see `count`).
    pub fn refresh_landlords<'info>(ctx: Context<'info, RefreshLandlords<'info>>) -> Result<()> {
        instructions::count::handle_refresh_landlords(ctx)
    }

    /// Parameters: proposed by the admin, applied after 72 hours (by the admin
    /// for the first day, then by anyone, until the proposal expires).
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

    /// Timelocked: the first call proposes, a call 72 hours later retires.
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

    /// The refresher gives up its role at once; works after renounce too.
    pub fn resign_refresher(ctx: Context<ResignRefresher>) -> Result<()> {
        instructions::roles::handle_resign_refresher(ctx)
    }
}
