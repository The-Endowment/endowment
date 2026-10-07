//! Safety rules that remain enforced even if an admin or service signs a bad
//! instruction. No synthetic allowance is used in transition/accrual tests.
use super::*;

#[test]
fn mandatory_allowance_and_ceiling_apply_at_creation_and_proposal() {
    let mut env = Env::new();
    for (margin, ceiling) in [(0, MAX_BUY_PER_DAY), (10_000, 0), (10_000, 10 * MAX_BUY_PER_DAY + 1)] {
        let mut create = params(env.guardian.pubkey());
        create.params.allowance_margin_bps = margin;
        create.params.max_rewards_per_day = ceiling;
        assert_err!(env.create_with(create), InvalidAllowance);
    }
    env.create();
    let admin = env.admin();
    for (margin, ceiling) in [(0, MAX_BUY_PER_DAY), (10_000, 0), (10_000, 10 * MAX_BUY_PER_DAY + 1)] {
        let mut next = env.config_state().params;
        next.allowance_margin_bps = margin;
        next.max_rewards_per_day = ceiling;
        assert_err!(env.propose(&admin, next), InvalidAllowance);
    }
    env.change_params(|p| p.max_rewards_per_day = 10 * MAX_BUY_PER_DAY);
    // Lowering spending must also keep the rewards ceiling inside the ratio.
    let mut next = env.config_state().params;
    next.max_buy_per_day -= 1;
    assert_err!(env.propose(&admin, next), InvalidAllowance);
}

#[test]
fn public_launch_discards_founder_activation_and_needs_a_fresh_thirty_percent() {
    let mut env = Env::new();
    env.synthetic_allowance = false;
    env.create_active();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 720_000 * UNIT);
    let (owner, account) = env.registered_holder(0, 280_000 * UNIT);
    assert!(env.count());
    env.warp(DAY);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 2_800);
    assert!(env.config_state().active);
    let old_epoch = env.config_state().refresher_epoch;
    env.change_params(|p| {
        p.activate_bps = 3_000;
        p.deactivate_bps = 2_500;
    });
    let config = env.config_state();
    assert!(config.public_launched());
    assert!(!config.active && !config.reward_credit_ok && !config.count.open);
    assert_eq!((config.last_count_at, config.last_count_bps, config.last_committed), (0, 0, 0));
    assert_ne!(config.refresher_epoch, old_epoch);
    assert_eq!(config.carry_floor(env.now()), config.reward_index);
    env.airdrop_dividend(&account, 100);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotActive);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 2_800);
    assert!(!env.config_state().active, "founder hysteresis must not survive at 28%");
    let ix = env.coin_transfer_ix(&outsider.pubkey(), &owner.pubkey(), 20_000 * UNIT);
    assert!(send(&mut env.svm, &[ix], &outsider, &[&outsider]));
    for _ in 0..2 { env.warp(DAY); assert!(env.count()); }
    assert_eq!(env.committed_bps(), 3_000);
    assert!(env.config_state().active);
    let mut next = env.config_state().params;
    next.activate_bps = 0;
    next.deactivate_bps = 0;
    assert_err!(env.propose(&env.admin(), next), PublicLaunchLocked);
}

#[test]
fn public_launch_expires_live_founder_credit_but_allows_fresh_public_rewards() {
    let mut env = Env::new();
    env.synthetic_allowance = false;
    env.create_active();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 700_000 * UNIT);
    let (owner, account) = env.registered_holder(0, 300_000 * UNIT);
    assert!(env.count());
    env.warp(DAY);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 3_000);

    let post = |env: &mut Env, total: u64| {
        let signer = refresher();
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::PostRewardTotal { total }.data(),
            endowment::accounts::PostRewardTotal {
                refresher: signer.pubkey(), config: env.config(), coin_mint: env.inst.coin_mint,
            }.to_account_metas(None),
        );
        assert!(send(&mut env.svm, &[ix], &signer, &[&signer]));
    };
    let mut public_params = env.config_state().params;
    public_params.activate_bps = 3_000;
    public_params.deactivate_bps = 2_500;
    assert!(env.propose(&env.admin(), public_params));
    // Earn near the END of the 72-hour proposal wait. Credit earned before
    // proposing could expire naturally and would not test the launch reset.
    env.warp(PARAM_TIMELOCK_SECONDS - DAY);
    post(&mut env, 0);
    env.warp(DAY);
    post(&mut env, 1_000 * UNIT);
    assert!(env.count()); // Real count settles the posted founder allowance.
    let founder_config = env.config_state();
    let mut founder_credit = env.landlord_state(&owner.pubkey());
    assert!(founder_config.reward_index > 0);
    assert_eq!(founder_credit.allowance, 300 * UNIT);
    env.airdrop_dividend(&account, 1_000 * UNIT);

    assert!(env.apply_params());
    assert!(!env.config_state().active);
    assert_eq!(env.config_state().reward_index, founder_config.reward_index);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotActive);
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count()); // Fresh attestations and a new public 30% count.
    assert_eq!(env.committed_bps(), 3_000);
    assert!(env.config_state().active);

    // A counterfactual calculation on the untouched founder snapshots proves
    // this credit is still young enough to spend without the launch reset.
    // It does not write synthetic allowance or mutate any on-chain accounts.
    let old_floor = founder_config.carry_floor(env.now());
    assert!(old_floor < founder_config.reward_index);
    founder_credit.settle(founder_config.reward_index, old_floor);
    assert_eq!(founder_credit.allowance, 300 * UNIT);
    assert_eq!(env.landlord_state(&owner.pubkey()).allowance, 0);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT,
        "recent founder credit must not debit the wallet after public activation");
    assert_eq!(env.hold_state().pending, 0);

    // First public post rebases. Only a subsequent public distribution earns
    // a new allowance, and the real sweep can collect exactly that new share.
    post(&mut env, 1_000 * UNIT);
    assert_eq!(env.config_state().reward_index, founder_config.reward_index);
    env.warp(DAY);
    assert!(env.count());
    post(&mut env, 2_000 * UNIT);
    assert!(env.config_state().reward_index > founder_config.reward_index);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 700 * UNIT);
    assert_eq!(env.hold_state().pending, 300 * UNIT);
    assert_eq!(env.landlord_state(&owner.pubkey()).allowance, 0);
}

#[test]
fn apply_revalidates_stale_or_corrupt_pending_parameters() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    assert!(env.propose(&admin, env.config_state().params));
    env.warp(PARAM_TIMELOCK_SECONDS);
    // Simulates pending pre-upgrade state: validation must also happen when
    // executing, rather than trusting a once-valid serialized proposal.
    let mut config = env.config_state();
    config.pending.params.activate_bps = 0;
    config.pending.params.deactivate_bps = 0;
    env.poke(&env.config(), |data| config.try_serialize(&mut &mut data[..]).unwrap());
    assert_err!(env.apply_params(), PublicLaunchLocked);
    config.pending.params = config.params;
    config.pending.params.allowance_margin_bps = 0;
    env.poke(&env.config(), |data| config.try_serialize(&mut &mut data[..]).unwrap());
    assert_err!(env.apply_params(), InvalidAllowance);
}

#[test]
fn recovery_roles_cannot_be_removed_until_retirement_or_while_paused() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    assert_err!(env.renounce(&admin), RetirementRequired);
    assert_err!(env.set_guardian(&admin, Pubkey::default()), GuardianRequired);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    env.retire_now(&admin);
    assert_err!(env.renounce(&admin), Paused);
    assert!(env.unpause(&admin));
    assert!(env.renounce(&admin));
}

#[test]
fn initialization_cannot_be_bricked_by_renouncing_before_policy_creation() {
    let mut env = Env::new();
    let create = env.create_ix_for(&env.inst, params(env.guardian.pubkey()));
    let admin = env.admin();
    assert!(send(&mut env.svm, &[create], &admin, &[&admin]));
    assert!(!exists(&env.svm, &policy_pda(&env.config())));
    assert_err!(env.renounce(&admin), RetirementRequired);
    env.initialize_hold(&env.inst.clone());
    assert!(exists(&env.svm, &policy_pda(&env.config())));
}
