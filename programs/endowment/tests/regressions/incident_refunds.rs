//! An incident permanently cancels all still-pending receipts collected at or
//! before its timestamp. Later approval or resume cannot restore release.
use super::*;
use super::collection_hold::held;

#[test]
fn approved_receipts_are_refund_only_after_pause_and_restart() {
    let (mut env, owner, account) = held(100);
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    let cutoff = env.config_state().pause_started_at;
    let outsider = env.funded();
    assert_err!(env.settle_hold(&owner.pubkey(), 0, &outsider, true), Paused);
    assert!(env.unpause(&env.admin()));
    assert_eq!(env.config_state().pause_started_at, cutoff);
    assert_err!(env.settle_hold(&owner.pubkey(), 0, &outsider, true), CollectionInvalidated);
    assert!(env.settle_hold(&owner.pubkey(), 0, &outsider, false));
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);
    assert_eq!((env.hold_state().pending, env.hold_state().refunded), (0, 100));
    assert_eq!(env.config_state().total_swept, 0);
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 100);
    assert!(env.consent_state(&owner.pubkey()).enabled);
}

#[test]
fn unreviewed_incident_receipts_cannot_be_approved_during_or_after_pause() {
    let (mut env, owner, account) = held(100);
    assert!(env.pause(&env.guardian.insecure_clone()));
    env.warp(HOLD_SECONDS);
    assert_err!(env.review_hold(&owner.pubkey(), 0, 100), CollectionInvalidated);
    assert!(env.unpause(&env.admin()));
    assert_err!(env.review_hold(&owner.pubkey(), 0, 100), CollectionInvalidated);
    assert_err!(env.settle_hold(&owner.pubkey(), 0, &collector(), true), CollectionInvalidated);
    let outsider = env.funded();
    assert!(env.settle_hold(&owner.pubkey(), 0, &outsider, false));
    assert_eq!(token_balance(&env.svm, &account), 100);
}

#[test]
fn an_incident_permits_immediate_permissionless_refund_before_the_hold() {
    let (mut env, owner, account) = held(100);
    let outsider = env.funded();
    assert_err!(env.settle_hold(&owner.pubkey(), 0, &outsider, false), RefundNotAllowed);
    let receipt = env.receipt_state(&owner.pubkey(), 0);
    assert!(env.pause(&env.guardian.insecure_clone()));
    assert_eq!(receipt.collected_at, env.config_state().pause_started_at);
    assert!(env.now() < receipt.release_at);
    assert!(env.settle_hold(&owner.pubkey(), 0, &outsider, false));
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert!(env.config_state().is_paused(env.now()));
}

#[test]
fn same_second_resume_is_conservative_but_later_collections_can_release() {
    let (mut env, owner, account) = held(100);
    assert!(env.pause(&env.guardian.insecure_clone()));
    assert!(env.unpause(&env.admin()));
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));
    let second = env.receipt_state(&owner.pubkey(), 1);
    assert_eq!(second.collected_at, env.config_state().pause_started_at);
    let outsider = env.funded();
    assert!(env.settle_hold(&owner.pubkey(), 1, &outsider, false));
    env.warp(1);
    env.airdrop_dividend(&account, 20);
    assert!(env.sweep(&owner.pubkey(), &account));
    let third = env.receipt_state(&owner.pubkey(), 2);
    assert!(third.collected_at > env.config_state().pause_started_at);
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 2, 20));
    assert!(env.settle_hold(&owner.pubkey(), 2, &outsider, true));
    assert_eq!(env.hold_state().released, 20);
    assert_err!(env.review_hold(&owner.pubkey(), 0, 100), CollectionInvalidated);
    assert!(env.settle_hold(&owner.pubkey(), 0, &outsider, false));
    assert_eq!(token_balance(&env.svm, &account), 150);
}

#[test]
fn successive_pauses_cancel_the_new_receipts_without_reviving_old_ones() {
    let (mut env, owner, account) = held(100);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    let first = env.config_state().pause_started_at;
    assert!(env.unpause(&env.admin()));
    env.warp(1);
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert!(env.pause(&guardian));
    assert!(env.config_state().pause_started_at > first);
    assert!(env.unpause(&env.admin()));
    env.warp(HOLD_SECONDS);
    for (nonce, amount) in [(0, 100), (1, 50)] {
        assert_err!(env.review_hold(&owner.pubkey(), nonce, amount), CollectionInvalidated);
        assert!(env.settle_hold(&owner.pubkey(), nonce, &collector(), false));
    }
    assert_eq!(token_balance(&env.svm, &account), 150);
    assert_eq!(env.hold_state().released, 0);
}

#[test]
fn a_repeated_pause_never_moves_the_incident_cutoff_backwards() {
    let (mut env, owner, _) = held(100);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    let first = env.config_state().pause_started_at;
    assert!(env.unpause(&env.admin()));
    assert!(env.unpause(&env.admin())); // A redundant resume cannot clear it.
    env.warp(-1);
    assert!(env.unpause(&env.admin())); // Clear the now-future resume timestamp.
    assert!(env.pause(&guardian));
    assert_eq!(env.config_state().pause_started_at, first);
    assert!(env.unpause(&env.admin()));
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), false));
}
