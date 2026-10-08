//! Count freshness is mandatory even in founders mode. These regressions use
//! actual attestations, counts, reward posts and sweeps, never seeded credit.
use super::*;

fn post(env: &mut Env, total: u64) {
    let signer = refresher();
    let ix = Instruction::new_with_bytes(
        endowment::id(), &endowment::instruction::PostRewardTotal { total }.data(),
        endowment::accounts::PostRewardTotal {
            refresher: signer.pubkey(), config: env.config(), coin_mint: env.inst.coin_mint,
        }.to_account_metas(None),
    );
    assert!(send(&mut env.svm, &[ix], &signer, &[&signer]));
}

fn counted(public: bool) -> (Env, Keypair, Pubkey) {
    let mut env = Env::new();
    env.synthetic_allowance = false;
    if public { env.create(); } else { env.create_active(); }
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 700_000 * UNIT);
    let (owner, account) = env.registered_holder(0, 300_000 * UNIT);
    assert!(env.count());
    env.warp(DAY);
    assert!(env.count());
    assert!(env.config_state().collecting(env.now()));
    (env, owner, account)
}

#[test]
fn founders_require_a_completed_count_before_collection_or_credit() {
    let mut env = Env::new();
    env.synthetic_allowance = false;
    env.create_active();
    let (owner, account) = env.registered_holder(0, 300_000 * UNIT);
    env.airdrop_dividend(&account, 100 * UNIT);
    assert!(env.config_state().active, "founders bypass participation only");
    assert_eq!(env.config_state().last_count_at, 0);
    post(&mut env, 0);
    env.warp(DAY);
    post(&mut env, 1_000 * UNIT);
    assert_eq!(env.config_state().reward_index, 0);
    assert_err!(env.sweep(&owner.pubkey(), &account), CountStale);
    assert!(env.begin());
    assert!(env.count_batch(&[owner.pubkey()]));
    assert_err!(env.sweep(&owner.pubkey(), &account), CountStale);
    assert!(env.finish());
    assert!(env.config_state().collecting(env.now()));
    assert!(!env.config_state().reward_credit_ok);
}

#[test]
fn both_modes_stop_at_the_count_age_boundary_even_with_live_reward_posts() {
    for public in [false, true] {
        let (mut env, owner, account) = counted(public);
        env.airdrop_dividend(&account, 5_000 * UNIT);
        post(&mut env, 0);
        env.warp(DAY);
        post(&mut env, 0);
        env.warp(ACTIVE_MAX_AGE_SECS - DAY);
        post(&mut env, 1_000 * UNIT);
        assert!(env.config_state().collecting(env.now()));
        assert!(env.sweep(&owner.pubkey(), &account));
        assert_eq!(env.hold_state().pending, 300 * UNIT);
        let index = env.config_state().reward_index;
        env.warp(1);
        assert_err!(env.sweep(&owner.pubkey(), &account), CountStale);
        env.warp(DAY);
        post(&mut env, 2_000 * UNIT);
        assert_eq!(env.config_state().reward_index, index);
        assert!(!env.config_state().reward_credit_ok);
        assert_err!(env.sweep(&owner.pubkey(), &account), CountStale);
        // A fresh receipt from the final eligible second remains reclaimable
        // while counts are stale; this is before that receipt's own expiry.
        assert!(env.settle_hold(&owner.pubkey(), 0, &owner, false));
        assert_eq!(token_balance(&env.svm, &account), 5_000 * UNIT);
        assert!(!env.consent_state(&owner.pubkey()).enabled);
    }
}

#[test]
fn recount_drops_outage_rewards_with_or_without_posts_during_the_outage() {
    for public in [false, true] {
        for keep_posting in [false, true] {
            let (mut env, owner, account) = counted(public);
            env.airdrop_dividend(&account, 5_000 * UNIT);
            post(&mut env, 0);
            env.warp(DAY);
            post(&mut env, 1_000 * UNIT);
            assert!(env.sweep(&owner.pubkey(), &account));
            assert_eq!(env.hold_state().pending, 300 * UNIT);
            let index = env.config_state().reward_index;
            env.warp(ACTIVE_MAX_AGE_SECS);
            assert_err!(env.sweep(&owner.pubkey(), &account), CountStale);
            if keep_posting {
                post(&mut env, 2_000 * UNIT);
                env.warp(DAY);
                post(&mut env, 3_000 * UNIT);
                assert_eq!(env.config_state().reward_index, index);
            }
            assert!(env.count());
            assert!(env.config_state().collecting(env.now()));
            assert!(!env.config_state().reward_credit_ok,
                "recount resets credit even if no post observed the outage");
            post(&mut env, 4_000 * UNIT);
            assert_eq!(env.config_state().reward_index, index);
            assert!(env.sweep(&owner.pubkey(), &account));
            assert_eq!(token_balance(&env.svm, &account), 4_700 * UNIT,
                "the first recovery post only establishes a new baseline");
            env.warp(DAY);
            assert!(env.count());
            post(&mut env, 5_000 * UNIT);
            assert!(env.sweep(&owner.pubkey(), &account));
            assert_eq!(token_balance(&env.svm, &account), 4_400 * UNIT,
                "only the fresh post-recovery distribution adds allowance");
        }
    }
}
