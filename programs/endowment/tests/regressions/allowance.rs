//! The reward allowance: a sweep never takes more than what the landlord's
//! counted coin earned, per the reward totals the refresher posts.
use super::*;

const MARGIN_BPS: u16 = ALLOWANCE_MARGIN_BPS;
const DAY: i64 = 24 * 60 * 60;

impl Env {
    fn post_ix(&self, total: u64, signer: &Pubkey) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::PostRewardTotal { total }.data(),
            endowment::accounts::PostRewardTotal {
                refresher: *signer,
                config: self.config(),
                coin_mint: self.inst.coin_mint,
            }
            .to_account_metas(None),
        )
    }

    fn post_by(&mut self, total: u64, signer: &Keypair) -> bool {
        let ix = self.post_ix(total, &signer.pubkey());
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    /// The refresher posts the coin's cumulative reward total.
    fn post(&mut self, total: u64) -> bool {
        let signer = refresher();
        self.post_by(total, &signer)
    }

    fn coin_supply(&self) -> u64 {
        let data = self.svm.get_account(&self.inst.coin_mint).unwrap().data;
        u64::from_le_bytes(data[36..44].try_into().unwrap())
    }

    /// What `counted` coin units earn when holders were paid `paid` in total.
    fn earned(&self, counted: u64, paid: u64) -> u64 {
        let grown = paid as u128 * MARGIN_BPS as u128 * REWARD_INDEX_SCALE / (self.coin_supply() as u128 * 10_000);
        (counted as u128 * grown / REWARD_INDEX_SCALE) as u64
    }
}

/// The counted landlords of `counted_env`, with the allowance on and two
/// counts done, so they count and sweeps are on.
fn allowance_env(max_rewards_per_day: u64) -> (Env, Vec<Keypair>) {
    let (mut env, owners) = counted_env();
    env.change_params(|p| {
        p.allowance_margin_bps = MARGIN_BPS;
        p.max_rewards_per_day = max_rewards_per_day;
    });
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(env.config_state().active);
    (env, owners)
}

#[test]
fn a_sweep_takes_at_most_what_the_coin_earned() {
    let (mut env, owners) = allowance_env(1_000_000 * UNIT);
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    let counted = env.landlord_state(&owner).counted_amount;
    assert!(counted > 0);

    assert!(env.post(0));
    env.warp(DAY);
    // 1,000 PUMP lands in the wallet: some rewards, the rest bought.
    env.airdrop_dividend(&account, 1_000 * UNIT);
    // Holders as a whole earned 1,000 PUMP; this landlord holds a tenth.
    assert!(env.post(1_000 * UNIT));
    let first = env.earned(counted, 1_000 * UNIT);
    assert_eq!(first, 100 * UNIT, "a tenth of 1,000, and no more");

    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT - first, "the rest stays with the landlord");
    assert_eq!(env.landlord_state(&owner).allowance, 0);
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT - first, "nothing more until more is earned");

    env.warp(DAY);
    assert!(env.post(2_000 * UNIT));
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT - 2 * first);
}

#[test]
fn the_baseline_still_protects_what_the_landlord_held_on_joining() {
    let (mut env, owners) = allowance_env(1_000_000 * UNIT);
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    // Re-join with 500 PUMP already in the wallet: that becomes the baseline.
    env.airdrop_dividend(&account, 500 * UNIT);
    let resync = env.resync_ix(&owner);
    assert!(send(&mut env.svm, &[resync], &owners[0], &[&owners[0]]));
    assert!(env.post(0));
    env.warp(DAY);
    assert!(env.post(1_000 * UNIT));
    // Allowance is 100 PUMP, but nothing sits above the baseline.
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 500 * UNIT);
    // The allowance waits for rewards that do arrive.
    env.airdrop_dividend(&account, 40 * UNIT);
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 500 * UNIT);
    assert_eq!(env.landlord_state(&owner).allowance, 60 * UNIT);
}

#[test]
fn opting_back_in_starts_the_allowance_afresh() {
    let (mut env, owners) = allowance_env(1_000_000 * UNIT);
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    assert!(env.post(0));
    env.warp(DAY);
    assert!(env.post(1_000 * UNIT));
    // Earned 100 PUMP of allowance, never swept (say, while away). On opting
    // back in, the website resyncs: that allowance is gone, so PUMP bought
    // afterwards can't be taken against it.
    let resync = env.resync_ix(&owner);
    assert!(send(&mut env.svm, &[resync], &owners[0], &[&owners[0]]));
    let landlord = env.landlord_state(&owner);
    assert_eq!((landlord.allowance, landlord.index_at), (0, env.config_state().reward_index));
    env.airdrop_dividend(&account, 500 * UNIT);
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 500 * UNIT);
}

#[test]
fn nothing_accrues_while_contributions_are_paused() {
    let (mut env, owners) = allowance_env(1_000_000 * UNIT);
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    assert!(env.post(0));
    env.warp(MIN_REWARD_POST_SPACING_SECS);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    let index = env.config_state().reward_index;
    assert!(env.post(1_000 * UNIT));
    assert_eq!(env.config_state().reward_index, index, "rewards paid while paused stay with landlords");
    let admin = env.admin();
    assert!(env.unpause(&admin));
    env.warp(MIN_REWARD_POST_SPACING_SECS);
    // The same total again: nothing new was paid since.
    assert!(env.post(1_000 * UNIT));
    assert_eq!(env.config_state().reward_index, index);
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT);
}

#[test]
fn a_post_is_clamped_to_the_daily_ceiling_and_a_lower_total_rebases() {
    let (mut env, owners) = allowance_env(100 * UNIT);
    let owner = owners[0].pubkey();
    let counted = env.landlord_state(&owner).counted_amount;
    assert!(env.post(0));
    env.warp(DAY);
    // A wildly high total is credited only at the ceiling's rate.
    assert!(env.post(1_000_000 * UNIT));
    let account = env.inst.dividend_account(&owner);
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    let clamped = env.earned(counted, 100 * UNIT);
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT - clamped);

    // A lower total (a corrected feed) re-bases without crediting anything,
    // so a bad high post can't block later honest ones.
    env.warp(MIN_REWARD_POST_SPACING_SECS);
    let index = env.config_state().reward_index;
    assert!(env.post(5 * UNIT));
    let config = env.config_state();
    assert_eq!((config.reward_index, config.last_reward_total), (index, 5 * UNIT));
}

#[test]
fn only_the_refresher_posts_and_not_too_often() {
    let (mut env, _) = allowance_env(1_000_000 * UNIT);
    let stranger = env.funded();
    assert_err!(env.post_by(0, &stranger), NotRefresher);
    assert!(env.post(0));
    env.warp(MIN_REWARD_POST_SPACING_SECS - 1);
    assert_err!(env.post(1), RewardPostTooSoon);
    env.warp(1);
    assert!(env.post(1));
}

#[test]
fn a_new_landlord_earns_only_once_it_is_counted() {
    let (mut env, _) = allowance_env(1_000_000 * UNIT);
    let (newcomer, account) = env.registered_holder(0, 10_000 * UNIT);
    assert!(env.post(0));
    env.warp(DAY);
    assert!(env.post(1_000 * UNIT));
    env.airdrop_dividend(&account, 100 * UNIT);
    assert!(env.sweep(&newcomer.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT, "not counted yet, so nothing earned");
}

#[test]
fn each_stretch_is_credited_at_the_amount_counted_during_it() {
    let (mut env, owners) = allowance_env(1_000_000 * UNIT);
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    let before = env.landlord_state(&owner).counted_amount;
    assert!(env.post(0));
    env.warp(DAY);
    assert!(env.post(1_000 * UNIT));

    // Half its coin moves to another wallet; the next count credits half.
    let (other, _) = env.new_landlord(0);
    let ix = env.coin_transfer_ix(&owner, &other.pubkey(), before / 2);
    assert!(send(&mut env.svm, &[ix], &owners[0], &[&owners[0]]));
    assert!(env.count());
    let after = env.landlord_state(&owner).counted_amount;
    assert_eq!(after, before / 2);

    env.warp(DAY);
    assert!(env.post(2_000 * UNIT));
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    let credited = env.earned(before, 1_000 * UNIT) + env.earned(after, 1_000 * UNIT);
    // What carries over is held to what the coin counted now would have earned
    // over the same days, so a landlord whose coin fell keeps a little less.
    let expected = credited.min(env.earned(after, 2_000 * UNIT));
    assert!(expected < credited);
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT - expected);
}

#[test]
fn unused_allowance_carries_over_for_three_days_and_no_longer() {
    let (mut env, owners) = allowance_env(1_000_000 * UNIT);
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    let counted = env.landlord_state(&owner).counted_amount;
    assert!(env.post(0));
    // Five days of rewards, 1,000 PUMP a day to holders as a whole, and the
    // landlord spends its share each day: nothing is there to sweep.
    for day in 1..=5u64 {
        env.warp(DAY);
        assert!(env.count());
        assert!(env.post(day * 1_000 * UNIT));
    }
    // It then buys 1,000 PUMP. Only the last three days' allowance is left.
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    let kept = 3 * env.earned(counted, 1_000 * UNIT);
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT - kept);
    assert_eq!(env.landlord_state(&owner).allowance, 0);
}

#[test]
fn with_the_margin_at_zero_sweeps_take_everything_above_the_baseline() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 0);
}
