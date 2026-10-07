//! A wallet omitted from current participation cannot spend a stale balance's
//! reward allowance. These tests use real counts and reward posts throughout.
use super::*;

fn post(env: &mut Env, total: u64) -> bool {
    let signer = refresher();
    let ix = Instruction::new_with_bytes(
        endowment::id(), &endowment::instruction::PostRewardTotal { total }.data(),
        endowment::accounts::PostRewardTotal {
            refresher: signer.pubkey(), config: env.config(), coin_mint: env.inst.coin_mint,
        }.to_account_metas(None),
    );
    send(&mut env.svm, &[ix], &signer, &[&signer])
}

fn two_holders() -> (Env, Keypair, Pubkey, Keypair, Pubkey, Keypair) {
    let mut env = Env::new();
    env.synthetic_allowance = false;
    env.create_active();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 600_000 * UNIT);
    let (a, a_dividend) = env.registered_holder(0, 100_000 * UNIT);
    let (b, b_dividend) = env.registered_holder(0, 300_000 * UNIT);
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 4_000);
    (env, a, a_dividend, b, b_dividend, outsider)
}

fn finish_without(env: &mut Env, included: Pubkey) {
    assert!(env.attest_fully(&[included]));
    let begin = env.begin_ix(env.config());
    assert!(env.crank(&[begin]));
    assert!(env.count_batch(&[included]));
    env.warp(COUNT_TIMEOUT_SECS);
    assert!(env.finish());
}

#[test]
fn public_launch_omitted_founder_cannot_earn_from_a_retired_balance() {
    let (mut env, a, a_dividend, b, b_dividend, outsider) = two_holders();
    let old_epoch = env.landlord_state(&a.pubkey()).attestation_epoch;
    let transfer = env.coin_transfer_ix(&a.pubkey(), &outsider.pubkey(), 100_000 * UNIT);
    assert!(send(&mut env.svm, &[transfer], &a, &[&a]));
    env.change_params(|p| { p.activate_bps = 3_000; p.deactivate_bps = 2_500; });
    finish_without(&mut env, b.pubkey());
    assert!(env.config_state().active);
    assert_eq!(env.committed_bps(), 3_000);
    assert_ne!(env.config_state().refresher_epoch, old_epoch);
    assert_eq!(env.landlord_state(&a.pubkey()).counted_amount, 100_000 * UNIT);
    assert!(post(&mut env, 0));
    env.warp(COUNT_INTERVAL_SECS);
    assert!(post(&mut env, 1_000 * UNIT));
    env.airdrop_dividend(&a_dividend, 1_000 * UNIT);
    env.airdrop_dividend(&b_dividend, 1_000 * UNIT);
    assert!(env.sweep(&a.pubkey(), &a_dividend));
    assert_eq!(token_balance(&env.svm, &a_dividend), 1_000 * UNIT);
    assert_eq!(env.landlord_state(&a.pubkey()).allowance, 0);
    assert_eq!(env.landlord_state(&a.pubkey()).index_at, env.config_state().reward_index);
    assert!(env.sweep(&b.pubkey(), &b_dividend));
    assert_eq!(token_balance(&env.svm, &b_dividend), 700 * UNIT,
        "current, counted holdings still earn their exact share");
}

#[test]
fn timeout_omission_expires_credit_and_recount_does_not_backfill_it() {
    let (mut env, a, a_dividend, b, b_dividend, _) = two_holders();
    assert!(post(&mut env, 0));
    env.warp(COUNT_INTERVAL_SECS);
    assert!(post(&mut env, 1_000 * UNIT));
    let mut previously_eligible = env.landlord_state(&a.pubkey());
    let config = env.config_state();
    previously_eligible.settle(config.reward_index, config.carry_floor(env.now()));
    assert_eq!(previously_eligible.allowance, 100 * UNIT);
    finish_without(&mut env, b.pubkey());
    let config = env.config_state();
    assert_eq!(env.landlord_state(&a.pubkey()).attestation_epoch, config.refresher_epoch,
        "omission must fail closed even without an epoch change");
    assert!(env.landlord_state(&a.pubkey()).counted_round < config.count.round);
    // Do not settle/sweep A before recount: the count path itself must discard
    // both old allowance and index growth over the omitted period.
    env.warp(COUNT_INTERVAL_SECS);
    assert!(post(&mut env, 2_000 * UNIT));
    assert!(env.count());
    assert_eq!(env.landlord_state(&a.pubkey()).allowance, 0);
    env.airdrop_dividend(&a_dividend, 1_000 * UNIT);
    env.airdrop_dividend(&b_dividend, 1_000 * UNIT);
    assert!(env.sweep(&a.pubkey(), &a_dividend));
    assert_eq!(token_balance(&env.svm, &a_dividend), 1_000 * UNIT);
    assert!(env.sweep(&b.pubkey(), &b_dividend));
    assert_eq!(token_balance(&env.svm, &b_dividend), 400 * UNIT);
    env.warp(COUNT_INTERVAL_SECS);
    assert!(post(&mut env, 3_000 * UNIT));
    assert!(env.sweep(&a.pubkey(), &a_dividend));
    assert_eq!(token_balance(&env.svm, &a_dividend), 900 * UNIT,
        "a fresh recount earns future credit without restoring the stale gap");
}

#[test]
fn open_count_preserves_current_and_previous_round_allowances_until_finish() {
    let (mut env, a, a_dividend, b, b_dividend, _) = two_holders();
    assert!(post(&mut env, 0));
    env.warp(COUNT_INTERVAL_SECS);
    assert!(post(&mut env, 1_000 * UNIT));
    assert!(env.begin());
    assert!(env.count_batch(&[a.pubkey()]));
    assert_eq!(env.landlord_state(&a.pubkey()).counted_round, env.config_state().count.round);
    assert_eq!(env.landlord_state(&b.pubkey()).counted_round + 1, env.config_state().count.round);
    env.airdrop_dividend(&a_dividend, 1_000 * UNIT);
    env.airdrop_dividend(&b_dividend, 1_000 * UNIT);
    assert!(env.sweep(&a.pubkey(), &a_dividend));
    assert!(env.sweep(&b.pubkey(), &b_dividend));
    assert_eq!(token_balance(&env.svm, &a_dividend), 900 * UNIT);
    assert_eq!(token_balance(&env.svm, &b_dividend), 700 * UNIT);
}
