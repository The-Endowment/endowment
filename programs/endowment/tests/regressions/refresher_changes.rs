//! Refresher changes invalidate both open rounds and finalized eligibility.
use super::*;

fn replaced_refresher_cannot_finish_old_round(partial: bool, remove: bool) {
    let (mut env, owners) = counted_env();
    // The first two holders alone exceed 30%, so an incomplete round can
    // reactivate the campaign too if it survives until the timeout.
    env.mint_coin(&owners[0].pubkey(), 100_000 * UNIT);
    assert!(env.count());

    let mut next = env.config_state().params;
    next.refresher = if remove {
        Pubkey::default()
    } else {
        env.funded().pubkey()
    };
    assert!(env.propose(&env.admin(), next));
    env.warp(PARAM_TIMELOCK_SECONDS);

    let keys: Vec<_> = owners.iter().map(Signer::pubkey).collect();
    assert!(env.begin());
    assert!(env.count_batch(if partial { &keys[..2] } else { &keys }));
    let round = env.config_state().count;
    assert!(round.committed as u128 * 10_000 / round.supply as u128 >= 3_000);
    assert_eq!(round.counted < round.expected, partial);

    assert!(env.apply_params());
    assert!(!env.config_state().active);
    if partial {
        env.warp(COUNT_TIMEOUT_SECS);
    }
    let finished = env.finish();
    let account = env.inst.dividend_account(&owners[0].pubkey());
    env.airdrop_dividend(&account, 10 * UNIT);
    let swept = env.sweep(&owners[0].pubkey(), &account);
    assert!(
        !finished && !swept && token_balance(&env.svm, &account) == 10 * UNIT,
        "old round revived after refresher change: finished={finished}, swept={swept}"
    );
}

#[test]
fn audit_replacement_invalidates_complete_open_round() {
    replaced_refresher_cannot_finish_old_round(false, false);
}

#[test]
fn audit_replacement_invalidates_partial_open_round() {
    replaced_refresher_cannot_finish_old_round(true, false);
}

#[test]
fn audit_removal_invalidates_complete_open_round() {
    replaced_refresher_cannot_finish_old_round(false, true);
}

#[test]
fn audit_removal_invalidates_partial_open_round() {
    replaced_refresher_cannot_finish_old_round(true, true);
}

#[test]
fn audit_later_parameter_change_cannot_reuse_old_finalized_count() {
    let (mut env, _owners) = counted_env();
    assert!(env.count());
    let mut next = env.config_state().params;
    next.refresher = env.funded().pubkey();
    assert!(env.propose(&env.admin(), next));
    env.warp(PARAM_TIMELOCK_SECONDS);
    assert!(env.count());
    assert!(env.config_state().active);
    assert!(env.apply_params());
    assert!(!env.config_state().active);

    // No reads by the new refresher and no new count. A routine parameter
    // proposal must not reactivate the campaign using the retired count.
    env.change_params(|params| params.tip_bps = params.tip_bps.saturating_sub(1));
    assert!(
        !env.config_state().active,
        "routine parameter change reused the old count"
    );
}

#[test]
fn audit_new_refresher_can_restore_activity_with_fresh_attestations() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    let new_refresher = env.funded();
    env.change_params(|params| params.refresher = new_refresher.pubkey());
    let keys: Vec<_> = owners.iter().map(Signer::pubkey).collect();
    for pass in 0..REQUIRED_ATTESTATIONS {
        if pass > 0 {
            env.warp(MIN_ATTEST_SPACING_SECS);
        }
        let ix = env.refresh_ix_by(env.config(), &keys, &new_refresher.pubkey());
        assert!(send(&mut env.svm, &[ix], &new_refresher, &[&new_refresher]));
    }
    let begin = env.begin_ix(env.config());
    assert!(env.crank(&[begin]));
    assert!(env.count_batch(&keys));
    assert!(env.finish());
    assert!(env.config_state().active);
    assert_eq!(env.committed_bps(), 3_000);
}
