use super::*;
use super::reported_rewards::reporter_pda;

fn manage(env: &mut Env, caller: &Keypair, data: Vec<u8>) -> bool {
    let accounts = endowment::accounts::ManageReporter {
        caller: caller.pubkey(),
        config: env.config(),
        reporter_policy: reporter_pda(&env.config()),
    };
    let ix = Instruction::new_with_bytes(endowment::id(), &data, accounts.to_account_metas(None));
    send(&mut env.svm, &[ix], caller, &[caller])
}

#[test]
fn disabled_reporter_cannot_veto_timelocked_admin_recovery() {
    let mut env = Env::new();
    env.create_active();
    let admin = env.admin();
    let old_reporter = env.funded();
    let replacement = env.funded();
    let guardian = env.guardian.insecure_clone();
    let stranger = env.funded();

    assert!(manage(&mut env, &admin, endowment::instruction::ProposeReporter {
        reporter: old_reporter.pubkey(),
    }.data()));
    env.warp(PARAM_TIMELOCK_SECONDS);
    assert!(manage(&mut env, &admin, endowment::instruction::ApplyReporter {}.data()));
    // The guardian can use the existing bounded pause, but cannot stop the
    // reporter indefinitely and force an admin recovery proposal.
    assert_err!(manage(&mut env, &guardian, endowment::instruction::DisableReporter {}.data()), NotReporter);
    assert!(!env.policy().disabled);
    assert!(manage(&mut env, &admin, endowment::instruction::DisableReporter {}.data()));
    assert!(manage(&mut env, &admin, endowment::instruction::ProposeReporter {
        reporter: replacement.pubkey(),
    }.data()));

    let recovery_at = env.policy().effective_at;
    let disabled_epoch = env.policy().epoch;
    assert_err!(manage(&mut env, &stranger, endowment::instruction::DisableReporter {}.data()), NotReporter);
    assert_err!(manage(&mut env, &guardian, endowment::instruction::DisableReporter {}.data()), NotReporter);
    // Every authorized caller observes the same idempotent stop semantics.
    // In particular, a leaked reporter key cannot keep restarting recovery.
    for caller in [&old_reporter, &admin] {
        assert!(manage(&mut env, caller, endowment::instruction::DisableReporter {}.data()));
        let policy = env.policy();
        assert!(policy.disabled);
        assert_eq!(policy.epoch, disabled_epoch);
        assert_eq!((policy.pending, policy.effective_at), (replacement.pubkey(), recovery_at));
    }
    assert_err!(manage(&mut env, &admin, endowment::instruction::ApplyReporter {}.data()), TimelockNotElapsed);
    env.warp(PARAM_TIMELOCK_SECONDS - 1);
    assert!(manage(&mut env, &old_reporter, endowment::instruction::DisableReporter {}.data()));
    assert_err!(manage(&mut env, &admin, endowment::instruction::ApplyReporter {}.data()), TimelockNotElapsed);
    env.warp(1);
    assert!(manage(&mut env, &admin, endowment::instruction::ApplyReporter {}.data()));
    let policy = env.policy();
    assert!(!policy.disabled);
    assert_eq!(policy.reporter, replacement.pubkey());
    assert_eq!(policy.epoch, disabled_epoch + 1);
    assert_eq!((policy.pending, policy.effective_at), (Pubkey::default(), 0));
    assert_err!(manage(&mut env, &old_reporter, endowment::instruction::DisableReporter {}.data()), NotReporter);
}

#[test]
fn admin_can_explicitly_cancel_recovery_while_collection_is_disabled() {
    let mut env = Env::new();
    env.create_active();
    let admin = env.admin();
    let replacement = env.funded();
    assert!(manage(&mut env, &admin, endowment::instruction::DisableReporter {}.data()));
    assert!(manage(&mut env, &admin, endowment::instruction::ProposeReporter {
        reporter: replacement.pubkey(),
    }.data()));
    let epoch = env.policy().epoch;
    assert!(manage(&mut env, &admin, endowment::instruction::CancelReporter {}.data()));
    let policy = env.policy();
    assert!(policy.disabled);
    assert_eq!(policy.epoch, epoch);
    assert_eq!((policy.pending, policy.effective_at), (Pubkey::default(), 0));
    env.warp(PARAM_TIMELOCK_SECONDS);
    assert_err!(manage(&mut env, &admin, endowment::instruction::ApplyReporter {}.data()), NoPendingParams);
}
