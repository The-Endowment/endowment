//! The reward allowance: a landlord can be swept at most what its counted coin
//! earned.
//!
//! Dividend-paying coins pay holders pro rata, and their launchpads publish a
//! cumulative total paid to holders. Once a day or so the refresher posts that
//! total here. The increase since the last post, divided by the coin's supply
//! and multiplied by the endowment's margin, is what each coin base unit
//! earned; it accumulates in `Config::reward_index`, and each landlord's
//! allowance grows by its counted coin times the index's growth
//! (`Landlord::settle`). Sweeps take at most that allowance, and allowance a
//! landlord doesn't use carries over for at most three elapsed days of posted credit
//! (`Config::reward_marks`).
//!
//! Dividing by the whole supply understates what each eligible unit earned
//! (pools and other non-earning holders get nothing), which errs toward taking
//! less, never more. The margin is exactly 1.0x.
//!
//! The total is public and checkable, but the post is trusted: a refresher that
//! posts too high releases more allowance than earned. That is bounded:
//! - each post is credited at most `max_rewards_per_day` for the time since
//!   the last post, and posts are at least MIN_REWARD_POST_SPACING_SECS apart;
//! - nothing is credited while contributions aren't running (inactive, paused,
//!   stale count, retired, or the goal reached), so rewards paid then stay
//!   with landlords: a post credits only if they were running at the last
//!   post and still are, with no pause or switch-on in between;
//! - a post after a gap credits at most MAX_REWARD_CREDIT_SECS' share of the
//!   increase, and the ceiling covers at most that long;
//! - an allowance only ever lets a sweep take dividend above the landlord's
//!   baseline from its own delegated account, into the endowment's vault.
//!
//! A total lower than the last one (a corrected or reset feed) re-bases without
//! crediting anything, so a mistaken high post can't block later honest ones.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::{constants::*, error::EndowmentError, events::RewardTotalPosted, state::Config};

#[derive(Accounts)]
pub struct PostRewardTotal<'info> {
    pub refresher: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
        constraint = config.params.refresher != Pubkey::default()
            && config.params.refresher == refresher.key() @ EndowmentError::NotRefresher,
    )]
    pub config: Box<Account<'info, Config>>,
    /// Its supply is the denominator.
    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
}

pub fn handle_post_reward_total(ctx: Context<PostRewardTotal>, total: u64) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config_key = ctx.accounts.config.key();
    let supply = ctx.accounts.coin_mint.supply;
    let config = &mut ctx.accounts.config;

    let mut credited = 0;
    config.mark_rewards(now);
    if config.last_reward_post_at != 0 {
        let elapsed = now.saturating_sub(config.last_reward_post_at);
        require!(elapsed >= MIN_REWARD_POST_SPACING_SECS, EndowmentError::RewardPostTooSoon);
        let margin = config.params.allowance_margin_bps;
        // Contributions were running at the last post, are running now, and
        // no pause or switch-on fell in between.
        let running = config.reward_credit_ok
            && config.collecting(now)
            && config.paused_until <= config.last_reward_post_at;
        if margin > 0 && supply > 0 && running {
            // After a gap, only the last MAX_REWARD_CREDIT_SECS' share of the
            // increase is credited: older rewards stay with landlords.
            let window = elapsed.min(MAX_REWARD_CREDIT_SECS) as u128;
            let ceiling = (config.params.max_rewards_per_day as u128 * window / 86_400).min(u64::MAX as u128);
            let increase = total.saturating_sub(config.last_reward_total) as u128 * window / elapsed as u128;
            credited = increase.min(ceiling) as u64;
            // At most u64::MAX * ALLOWANCE_MARGIN_BPS * REWARD_INDEX_SCALE, well within u128.
            let grown = credited as u128 * margin as u128 * REWARD_INDEX_SCALE / (supply as u128 * 10_000);
            config.reward_index = config.reward_index.saturating_add(grown);
        }
    }
    // The first post only sets the starting point.
    config.last_reward_total = total;
    config.last_reward_post_at = now;
    config.reward_credit_ok = config.collecting(now);

    emit!(RewardTotalPosted { config: config_key, total, credited, reward_index: config.reward_index, at: now });
    Ok(())
}
