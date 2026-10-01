use super::*;
use endowment::{ReporterPolicy, RewardReport};

pub(super) fn reporter_pda(config: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"reporter", config.as_ref()], &endowment::id()).0
}

impl Env {
    pub(super) fn policy(&self) -> ReporterPolicy {
        let data = self.svm.get_account(&reporter_pda(&self.config())).unwrap().data;
        ReporterPolicy::try_deserialize(&mut &data[..]).unwrap()
    }
    pub(super) fn report(&self, accounts: &endowment::accounts::Sweep, amount: u64) -> RewardReport {
        let data = self.svm.get_account(&accounts.landlord).unwrap().data;
        let landlord = Landlord::try_deserialize(&mut &data[..]).unwrap();
        RewardReport {
            consent_id: landlord.consent_id, nonce: landlord.last_report_nonce + 1,
            collection_epoch: Self::config_at(&self.svm, &accounts.config).collection_epoch,
            reporter_epoch: self.policy().epoch, amount,
            expected_source_balance: token_balance(&self.svm, &accounts.dividend_account),
            issued_at: self.now(), expires_at: self.now() + 120, evidence_hash: [7; 32],
        }
    }
    pub(super) fn collect_ix(&self, accounts: endowment::accounts::Sweep, report: RewardReport, signer: Pubkey) -> Instruction {
        let config = accounts.config;
        Instruction::new_with_bytes(endowment::id(), &endowment::instruction::CollectReward { report }.data(),
            endowment::accounts::CollectReward { sweep: accounts, reporter_policy: reporter_pda(&config), reporter: signer }
            .to_account_metas(None))
    }
    pub(super) fn collect(&mut self, accounts: endowment::accounts::Sweep, report: RewardReport, signer: &Keypair) -> bool {
        let ix = self.collect_ix(accounts, report, signer.pubkey());
        send(&mut self.svm, &[ix], signer, &[signer])
    }
    fn explicit(&mut self, owner: &Pubkey, account: &Pubkey, amount: u64) -> bool {
        let accounts = self.sweep_accounts(owner, account);
        let report = self.report(&accounts, amount);
        let signer = self.inst.creator.insecure_clone();
        self.collect(accounts, report, &signer)
    }
    fn renew_consent(&mut self, owner: &Keypair) -> bool {
        let mut ix = self.resync_ix(&owner.pubkey());
        ix.data = endowment::instruction::RenewRewardConsent {}.data();
        send(&mut self.svm, &[ix], owner, &[owner])
    }
    fn reporter_action(&mut self, caller: &Keypair, data: Vec<u8>) -> bool {
        let ix = Instruction::new_with_bytes(endowment::id(), &data, endowment::accounts::ManageReporter {
            caller: caller.pubkey(), config: self.config(), reporter_policy: reporter_pda(&self.config()),
        }.to_account_metas(None));
        send(&mut self.svm, &[ix], caller, &[caller])
    }
}

#[test]
fn only_the_explicit_report_amount_moves_and_nonce_is_atomic() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(1_000);
    env.airdrop_dividend(&account, 600); // e.g. 200 before activation, 300 bought, 100 eligible.
    assert!(env.explicit(&owner.pubkey(), &account, 100));
    assert_eq!(token_balance(&env.svm, &account), 1_500);
    assert_eq!(env.landlord_state(&owner.pubkey()).last_report_nonce, 1);
    assert_eq!(env.config_state().total_swept, 100);
    assert_err!(env.explicit(&owner.pubkey(), &account, 0), CollectionLimit);
    assert_eq!(env.landlord_state(&owner.pubkey()).last_report_nonce, 1);
    // No baseline-based floor: the reporter determines eligibility after outflows.
    env.set_balance(&account, 70);
    assert!(env.explicit(&owner.pubkey(), &account, 70));
    assert_eq!(token_balance(&env.svm, &account), 0);
}

#[test]
fn legacy_collection_and_resync_are_disabled() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(100);
    env.airdrop_dividend(&account, 500);
    let signer = env.funded();
    let ix = Instruction::new_with_bytes(endowment::id(), &endowment::instruction::Sweep {}.data(),
        env.sweep_accounts(&owner.pubkey(), &account).to_account_metas(None));
    assert_err!(send(&mut env.svm, &[ix], &signer, &[&signer]), LegacyCollectionDisabled);
    let ix = env.resync_ix(&owner.pubkey());
    assert_err!(send(&mut env.svm, &[ix], &owner, &[&owner]), LegacyCollectionDisabled);
    let (new_owner, new_account) = env.new_landlord(0);
    let mut ix = env.register_ix(&new_owner.pubkey(), &new_account);
    ix.data = endowment::instruction::RegisterLandlord {}.data();
    let approve = env.approve_ix(&new_owner.pubkey(), &new_account);
    assert_err!(send(&mut env.svm, &[approve, ix], &new_owner, &[&new_owner]), LegacyCollectionDisabled);
    assert_eq!(token_balance(&env.svm, &account), 600);
}

#[test]
fn authentication_replay_expiry_and_source_change_fail_closed() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(1_000);
    let signer = env.admin();
    let accounts = env.sweep_accounts(&owner.pubkey(), &account);
    let report = env.report(&accounts, 100);
    let stranger = env.funded();
    assert_err!(env.collect(accounts, report.clone(), &stranger), NotReporter);
    for field in 0..7 {
        let mut bad = report.clone();
        match field {
            0 => bad.consent_id += 1, 1 => bad.collection_epoch += 1, 2 => bad.reporter_epoch += 1,
            3 => bad.nonce += 1, 4 => bad.issued_at += 1, 5 => bad.expires_at += 1,
            _ => bad.expected_source_balance += 1,
        }
        assert!(!env.collect(env.sweep_accounts(&owner.pubkey(), &account), bad, &signer));
    }
    assert_eq!(env.landlord_state(&owner.pubkey()).last_report_nonce, 0);
    assert!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), report.clone(), &signer));
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), report, &signer), ReportReplay);
    let report = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 50);
    env.warp(121);
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), report, &signer), ReportExpired);
}

#[test]
fn renewed_consent_invalidates_reports_without_a_daily_volume_limit() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(2_000_000 * UNIT);
    let old = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 100);
    assert!(env.renew_consent(&owner));
    let signer = env.admin();
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), old, &signer), StaleReport);
    // No per-wallet daily cap: repeated legitimate rewards can be collected
    // throughout the day, subject to actual allowance and treasury capacity.
    let amount = env.config_state().vault_cap();
    for _ in 0..4 {
        assert!(env.explicit(&owner.pubkey(), &account, amount));
        env.set_balance(&env.dividend_vault(), 0); // Simulate treasury consumption.
    }
    assert_eq!(env.landlord_state(&owner.pubkey()).total_contributed, 4 * amount);
}

#[test]
fn revoke_and_reenrollment_cannot_replay_prior_consent() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(1_000);
    let old = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 100);
    let signer = env.admin();
    env.revoke(&owner);
    assert_err!(env.explicit(&owner.pubkey(), &account, 100), NotDelegated);
    assert!(env.deregister(&owner));
    assert!(env.register(&owner));
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), old, &signer), StaleReport);
    assert!(env.explicit(&owner.pubkey(), &account, 100));
}

#[test]
fn pause_invalidates_reports_even_after_early_unpause() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(1_000);
    let old = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 100);
    let guardian = env.guardian.insecure_clone(); let admin = env.admin();
    assert!(env.pause(&guardian));
    assert_err!(env.explicit(&owner.pubkey(), &account, 100), Paused);
    assert!(env.renew_consent(&owner)); // Owner controls remain available.
    assert!(env.unpause(&admin));
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), old, &admin), StaleReport);
    assert!(env.explicit(&owner.pubkey(), &account, 100));
}

#[test]
fn reporter_rotation_and_emergency_disable_observe_timelock() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(1_000);
    let old = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 100);
    let admin = env.admin(); let next = env.funded();
    let propose = endowment::instruction::ProposeReporter { reporter: next.pubkey() }.data();
    assert_err!(env.reporter_action(&next, propose.clone()), NotAdmin);
    assert!(env.reporter_action(&admin, propose));
    assert_err!(env.reporter_action(&admin, endowment::instruction::ApplyReporter {}.data()), TimelockNotElapsed);
    env.warp(PARAM_TIMELOCK_SECONDS);
    assert_err!(env.reporter_action(&next, endowment::instruction::ApplyReporter {}.data()), ApplyGrace);
    assert!(env.reporter_action(&admin, endowment::instruction::ApplyReporter {}.data()));
    assert_err!(env.explicit(&owner.pubkey(), &account, 100), NotReporter);
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), old, &next), StaleReport);
    let fresh = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 100);
    assert!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), fresh, &next));
    assert!(env.reporter_action(&next, endowment::instruction::DisableReporter {}.data()));
    let fresh = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 100);
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), fresh, &next), NotReporter);
    assert_err!(env.reporter_action(&admin, endowment::instruction::ApplyReporter {}.data()), NoPendingParams);
}

#[test]
fn renunciation_freezes_pending_reporter_replacement() {
    let mut env = Env::new(); env.create();
    let admin = env.admin(); let next = env.funded();
    assert!(env.reporter_action(&admin, endowment::instruction::ProposeReporter { reporter: next.pubkey() }.data()));
    assert!(env.renounce(&admin));
    env.warp(PARAM_TIMELOCK_SECONDS + PARAM_APPLY_GRACE_SECONDS);
    assert_err!(env.reporter_action(&next, endowment::instruction::ApplyReporter {}.data()), ReporterFrozen);
    assert_eq!(env.policy().reporter, admin.pubkey());
    assert!(env.reporter_action(&admin, endowment::instruction::DisableReporter {}.data()));
    assert!(env.policy().disabled);
}

#[test]
fn source_balance_and_version_preconditions_and_capacity_reject_without_consuming_nonce() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(1_000);
    let signer = env.admin();
    let old = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 100);
    env.airdrop_dividend(&account, 1);
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), old, &signer), SourceBalanceChanged);
    assert_err!(env.explicit(&owner.pubkey(), &account, 1_002), CollectionLimit);
    let cap = env.config_state().vault_cap();
    env.set_balance(&env.dividend_vault(), cap);
    assert_err!(env.explicit(&owner.pubkey(), &account, 1), CollectionLimit);
    env.set_balance(&env.dividend_vault(), 0);
    let landlord = landlord_pda(&env.config(), &owner.pubkey());
    env.poke(&landlord, |data| data[8] = 3);
    assert_err!(env.explicit(&owner.pubkey(), &account, 1), UnsupportedCollectionVersion);
    assert_eq!(env.landlord_state(&owner.pubkey()).last_report_nonce, 0);
    assert_eq!(token_balance(&env.svm, &account), 1_001);
}

#[test]
fn count_completion_and_parameter_changes_invalidate_prepared_reports() {
    let mut env = Env::new(); env.create_active();
    let (owner, account) = env.registered_landlord(1_000);
    let signer = env.admin();
    let old = env.report(&env.sweep_accounts(&owner.pubkey(), &account), 100);
    assert!(env.begin());
    env.warp(COUNT_TIMEOUT_SECS);
    assert!(env.finish());
    // Refresh timestamps so this specifically tests epoch invalidation.
    let mut old = old; old.issued_at = env.now(); old.expires_at = env.now() + 120;
    assert_err!(env.collect(env.sweep_accounts(&owner.pubkey(), &account), old, &signer), StaleReport);
    let epoch = env.config_state().collection_epoch;
    env.change_params(|p| p.max_buy_per_day += 1);
    assert!(env.config_state().collection_epoch > epoch);
}
