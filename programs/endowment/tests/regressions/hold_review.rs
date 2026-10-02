//! Regressions from the review of PR #5 (found and fixed in #6), first reproduced against 1aca04c.
use super::collection_hold::{held, reviewer};
use super::*;

#[test]
fn expired_receipt_must_not_reopen_after_a_new_pause() {
    let (mut env, owner, _) = held(100);
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    env.warp(REFUND_SECONDS - HOLD_SECONDS + 1);
    assert_err!(
        env.settle_hold(&owner.pubkey(), 0, &collector(), true),
        CollectionExpired
    );

    // Both calls happen after the receipt has already become refund-only.
    let guardian = env.guardian.insecure_clone();
    let admin = env.admin();
    assert!(env.pause(&guardian));
    assert!(env.unpause(&admin));
    assert_err!(
        env.settle_hold(&owner.pubkey(), 0, &collector(), true),
        CollectionExpired
    );
}

fn post_total(env: &mut Env, total: u64) {
    let signer = refresher();
    let ix = Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::PostRewardTotal { total }.data(),
        endowment::accounts::PostRewardTotal {
            refresher: signer.pubkey(),
            config: env.config(),
            coin_mint: env.inst.coin_mint,
        }
        .to_account_metas(None),
    );
    assert!(send(&mut env.svm, &[ix], &signer, &[&signer]));
}

#[test]
fn three_day_allowance_must_expire_when_reward_posts_stop() {
    let (mut env, owners) = counted_env();
    env.change_params(|p| {
        p.allowance_margin_bps = ALLOWANCE_MARGIN_BPS;
        p.max_rewards_per_day = 1_000_000 * UNIT;
    });
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    post_total(&mut env, 0);
    env.warp(86_400);
    assert!(env.count());
    post_total(&mut env, 1_000 * UNIT);

    // Counting stays fresh, but the separate reward-post service has an outage.
    for _ in 0..5 {
        env.warp(86_400);
        assert!(env.count());
    }
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    // Bought PUMP appears after the old credit should have expired. Exercise
    // the on-chain bound independently of the collector's correctness.
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    assert_eq!(
        token_balance(&env.svm, &account),
        1_000 * UNIT,
        "allowance older than three elapsed days must not authorize a debit"
    );
}

#[test]
fn old_enrollment_refund_must_not_erase_new_enrollment_contributions() {
    let (mut env, owner, account) = held(100);
    assert!(env.deregister(&owner));
    // Same timestamp: enrollment identity must not depend on wall-clock time.
    assert!(env.register(&owner));
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(env.landlord_state(&owner.pubkey()).total_contributed, 50);

    // The old 100 was never counted in this newly created landlord record.
    assert!(env.settle_hold(&owner.pubkey(), 0, &reviewer(), false));
    assert_eq!(env.config_state().total_swept, 50);
    assert_eq!(env.hold_state().pending, 50);
    assert_eq!(
        env.landlord_state(&owner.pubkey()).total_contributed,
        50,
        "old-epoch refunds must not subtract this enrollment's contributions"
    );
}

#[test]
fn renewed_consent_refund_still_adjusts_the_same_enrollment_total() {
    let (mut env, owner, account) = held(100);
    assert!(env.enable_hold(&owner));
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(env.landlord_state(&owner.pubkey()).total_contributed, 150);
    assert!(env.settle_hold(&owner.pubkey(), 0, &reviewer(), false));
    assert_eq!(env.landlord_state(&owner.pubkey()).total_contributed, 50);
    assert!(env.consent_state(&owner.pubkey()).enabled);
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 100);
}
