use super::*;

pub(super) fn policy_pda(config: &Pubkey) -> Pubkey {
    pda(&[POLICY_SEED, config.as_ref()])
}
pub(super) fn consent_pda(config: &Pubkey, owner: &Pubkey) -> Pubkey {
    pda(&[CONSENT_SEED, config.as_ref(), owner.as_ref()])
}
pub(super) fn receipt_pda(config: &Pubkey, owner: &Pubkey, nonce: u64) -> Pubkey {
    pda(&[RECEIPT_SEED, config.as_ref(), owner.as_ref(), &nonce.to_le_bytes()])
}
pub(super) fn collector() -> Keypair {
    Keypair::new_from_array([43; 32])
}
pub(super) fn reviewer() -> Keypair {
    Keypair::new_from_array([44; 32])
}

impl Env {
    pub(super) fn initialize_hold(&mut self, inst: &Inst) {
        let admin = inst.creator.insecure_clone();
        let policy = policy_pda(&inst.config());
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::InitializeCollection {
                collector: collector().pubkey(),
                reviewer: reviewer().pubkey(),
            }
            .data(),
            endowment::accounts::InitializeCollection {
                admin: admin.pubkey(),
                config: inst.config(),
                policy,
                dividend_mint: inst.dividend_mint,
                pending_vault: ata(&policy, &inst.dividend_mint, &inst.dividend_program),
                dividend_token_program: inst.dividend_program,
                associated_token_program: ATA_PROGRAM,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        assert!(send(&mut self.svm, &[ix], &admin, &[&admin]));
        self.svm.airdrop(&collector().pubkey(), 10_000_000_000).unwrap();
        self.svm.airdrop(&reviewer().pubkey(), 10_000_000_000).unwrap();
    }
    pub(super) fn pending_vault(&self) -> Pubkey {
        ata(
            &policy_pda(&self.config()),
            &self.inst.dividend_mint,
            &self.inst.dividend_program,
        )
    }
    pub(super) fn read_consent(&self, key: &Pubkey) -> CollectionConsent {
        CollectionConsent::try_deserialize(&mut self.svm.get_account(key).unwrap().data.as_slice()).unwrap()
    }
    pub(super) fn consent_state(&self, owner: &Pubkey) -> CollectionConsent {
        self.read_consent(&consent_pda(&self.config(), owner))
    }
    pub(super) fn receipt_state(&self, owner: &Pubkey, nonce: u64) -> PendingCollection {
        PendingCollection::try_deserialize(
            &mut self
                .svm
                .get_account(&receipt_pda(&self.config(), owner, nonce))
                .unwrap()
                .data
                .as_slice(),
        )
        .unwrap()
    }
    pub(super) fn hold_state(&self) -> CollectionPolicy {
        CollectionPolicy::try_deserialize(
            &mut self
                .svm
                .get_account(&policy_pda(&self.config()))
                .unwrap()
                .data
                .as_slice(),
        )
        .unwrap()
    }
    pub(super) fn enable_hold(&mut self, owner: &Keypair) -> bool {
        let config = self.config();
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::EnableCollection {}.data(),
            endowment::accounts::EnableCollection {
                owner: owner.pubkey(),
                config,
                policy: policy_pda(&config),
                consent: consent_pda(&config, &owner.pubkey()),
                landlord: landlord_pda(&config, &owner.pubkey()),
                dividend_account: self.inst.dividend_account(&owner.pubkey()),
            }
            .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], owner, &[owner])
    }
    pub(super) fn review_hold_as(&mut self, owner: &Pubkey, nonce: u64, amount: u64, signer: &Keypair) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::ReviewCollection {
                approved_amount: amount,
                evidence_hash: [2; 32],
            }
            .data(),
            endowment::accounts::ReviewCollection {
                reviewer: signer.pubkey(),
                config: self.config(),
                policy: policy_pda(&self.config()),
                receipt: receipt_pda(&self.config(), owner, nonce),
            }
            .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }
    pub(super) fn review_hold(&mut self, owner: &Pubkey, nonce: u64, amount: u64) -> bool {
        self.review_hold_as(owner, nonce, amount, &reviewer())
    }
    pub(super) fn settle_accounts(
        &self,
        owner: &Pubkey,
        nonce: u64,
        caller: &Pubkey,
    ) -> endowment::accounts::SettleCollection {
        let config = self.config();
        endowment::accounts::SettleCollection {
            caller: *caller,
            config,
            policy: policy_pda(&config),
            receipt: receipt_pda(&config, owner, nonce),
            consent: consent_pda(&config, owner),
            rent_recipient: collector().pubkey(),
            owner: *owner,
            landlord: landlord_pda(&config, owner),
            dividend_mint: self.inst.dividend_mint,
            pending_vault: self.pending_vault(),
            refund_account: self.inst.dividend_account(owner),
            authority: self.authority(),
            dividend_vault: self.dividend_vault(),
            coin_mint: self.inst.coin_mint,
            coin_vault: self.coin_vault(),
            dividend_token_program: self.inst.dividend_program,
            coin_token_program: self.inst.coin_program,
            associated_token_program: ATA_PROGRAM,
            system_program: system_program::ID,
        }
    }
    pub(super) fn settle_with(
        &mut self,
        accounts: endowment::accounts::SettleCollection,
        caller: &Keypair,
        release: bool,
    ) -> bool {
        let data = if release {
            endowment::instruction::ReleaseCollection {}.data()
        } else {
            endowment::instruction::RefundCollection {}.data()
        };
        let ix = Instruction::new_with_bytes(endowment::id(), &data, accounts.to_account_metas(None));
        send(&mut self.svm, &[ix], caller, &[caller])
    }
    /// The admin proposes (or, with `None`, applies) a change of collector and reviewer.
    pub(super) fn change_roles(&mut self, signer: &Keypair, roles: Option<(Pubkey, Pubkey)>) -> bool {
        let data = match roles {
            Some((collector, reviewer)) => endowment::instruction::ProposeCollectionRoles { collector, reviewer }.data(),
            None => endowment::instruction::ApplyCollectionRoles {}.data(),
        };
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &data,
            endowment::accounts::ChangeCollectionRoles {
                admin: signer.pubkey(),
                config: self.config(),
                policy: policy_pda(&self.config()),
            }
            .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }
    pub(super) fn settle_hold(&mut self, owner: &Pubkey, nonce: u64, caller: &Keypair, release: bool) -> bool {
        self.settle_with(self.settle_accounts(owner, nonce, &caller.pubkey()), caller, release)
    }
}

pub(super) fn held(amount: u64) -> (Env, Keypair, Pubkey) {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, amount);
    assert!(env.sweep(&owner.pubkey(), &account));
    (env, owner, account)
}

#[test]
fn holds_use_separate_custody_and_individual_timers_across_midnight() {
    let (mut env, owner, account) = held(1000);
    let first = env.receipt_state(&owner.pubkey(), 0);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);
    env.warp(3 * 3600);
    env.airdrop_dividend(&account, 500);
    assert!(env.sweep(&owner.pubkey(), &account));
    let second = env.receipt_state(&owner.pubkey(), 1);
    assert_eq!(second.release_at - first.release_at, 3 * 3600);
    env.warp(21 * 3600);
    assert!(env.review_hold(&owner.pubkey(), 0, 1000));
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), true));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 1000);
    assert_eq!(token_balance(&env.svm, &env.pending_vault()), 500);
    assert_eq!(env.hold_state().pending, 500);
    assert_err!(env.review_hold(&owner.pubkey(), 1, 500), HoldNotElapsed);
    assert_err!(env.settle_hold(&owner.pubkey(), 1, &collector(), true), HoldNotElapsed);
}

#[test]
fn release_requires_elapsed_hold_and_independent_review() {
    let (mut env, owner, _) = held(100);
    assert_err!(env.review_hold(&owner.pubkey(), 0, 100), HoldNotElapsed);
    env.warp(HOLD_SECONDS - 1);
    assert_err!(env.settle_hold(&owner.pubkey(), 0, &collector(), true), HoldNotElapsed);
    env.warp(1);
    assert_err!(
        env.settle_hold(&owner.pubkey(), 0, &collector(), true),
        CollectionNotReviewed
    );
    assert_err!(env.review_hold_as(&owner.pubkey(), 0, 100, &collector()), NotReviewer);
    assert_err!(env.review_hold(&owner.pubkey(), 0, 101), InvalidCollection);
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), true));
    assert!(!env.settle_hold(&owner.pubkey(), 0, &collector(), true));
    assert_eq!(env.hold_state().released, 100);
}

#[test]
fn reclaim_survives_pause_deregistration_and_requires_new_consent() {
    let (mut env, owner, account) = held(100);
    let epoch = env.consent_state(&owner.pubkey()).epoch;
    assert!(env.pause(&env.guardian.insecure_clone()));
    assert!(env.deregister(&owner));
    assert!(env.settle_hold(&owner.pubkey(), 0, &owner, false));
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert!(!env.consent_state(&owner.pubkey()).enabled);
    assert!(env.consent_state(&owner.pubkey()).epoch > epoch);
    assert!(env.unpause(&env.admin()));
    assert!(env.register(&owner)); // helper explicitly signs enable after register
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 100);
    assert_eq!(env.consent_state(&owner.pubkey()).next_nonce, 1);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100);
}

#[test]
fn partial_clearance_refunds_the_excess_and_protects_it_without_ending_consent() {
    let (mut env, owner, account) = held(100);
    assert_eq!(env.config_state().total_swept, 100);
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 60));
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), true));
    assert_eq!(token_balance(&env.svm, &account), 40);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 60);
    assert_eq!(env.hold_state().refunded, 40);
    // The holder didn't ask to stop, so it stays enrolled; what came back is
    // under its baseline, and the totals count only what was kept.
    assert!(env.consent_state(&owner.pubkey()).enabled);
    let landlord = env.landlord_state(&owner.pubkey());
    assert_eq!((landlord.baseline, landlord.total_contributed), (40, 60));
    assert_eq!(env.config_state().total_swept, 60);
    refresh_count(&mut env);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 40);
    // New dividend is still collected.
    env.airdrop_dividend(&account, 10);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 40);
    assert_eq!(env.hold_state().pending, 10);
}

/// Sweeps need a recent count; the hold tests warp past a day or more.
fn refresh_count(env: &mut Env) {
    assert!(env.count());
}

#[test]
fn a_reviewers_refund_leaves_consent_and_other_receipts_alone() {
    let (mut env, owner, account) = held(100);
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));
    let epoch = env.consent_state(&owner.pubkey()).epoch;
    // The reviewer rejects the first receipt outright, before its hold is up.
    assert!(env.settle_hold(&owner.pubkey(), 0, &reviewer(), false));
    let consent = env.consent_state(&owner.pubkey());
    assert!(consent.enabled && consent.epoch == epoch);
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 100);
    assert_eq!(token_balance(&env.svm, &account), 100);
    // The second is still reviewed and released as usual.
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 1, 50));
    assert!(env.settle_hold(&owner.pubkey(), 1, &collector(), true));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 50);
    assert_eq!((env.hold_state().released, env.hold_state().refunded), (50, 100));
}

#[test]
fn a_pause_does_not_run_out_the_time_to_review() {
    let (mut env, owner, _) = held(100);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    env.warp(MAX_PAUSE_SECONDS);
    // Seven days on, the pause has just ended: the receipt hasn't expired, so
    // nobody but the holder or the reviewer can send it back,
    assert_err!(env.settle_hold(&owner.pubkey(), 0, &collector(), false), RefundNotAllowed);
    env.warp(REFUND_SECONDS - HOLD_SECONDS - 1);
    // and the reviewer has the usual 48 hours from then.
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), true));
    assert_eq!(env.hold_state().released, 100);
}

#[test]
fn a_paused_receipt_still_expires_once_the_window_after_the_pause_is_over() {
    let (mut env, owner, account) = held(100);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    env.warp(MAX_PAUSE_SECONDS + REFUND_SECONDS - HOLD_SECONDS);
    assert_err!(env.review_hold(&owner.pubkey(), 0, 100), CollectionExpired);
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), false));
    assert_eq!(token_balance(&env.svm, &account), 100);
}

#[test]
fn the_admin_replaces_the_collector_and_reviewer_after_the_timelock() {
    let (mut env, owner, account) = held(100);
    let admin = env.admin();
    let stranger = env.funded();
    let (new_collector, new_reviewer) = (env.funded(), env.funded());
    let roles = Some((new_collector.pubkey(), new_reviewer.pubkey()));
    assert_err!(env.change_roles(&stranger, roles), NotAdmin);
    assert_err!(env.change_roles(&admin, Some((new_collector.pubkey(), new_collector.pubkey()))), InvalidCollectionPolicy);
    assert_err!(env.change_roles(&admin, None), NoPendingParams);
    assert!(env.change_roles(&admin, roles));
    assert_err!(env.change_roles(&admin, None), TimelockNotElapsed);
    // Until it is applied the old keys still hold the roles: a day before the
    // timelock ends the old collector collects again.
    env.warp(PARAM_TIMELOCK_SECONDS - HOLD_SECONDS);
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));
    env.warp(HOLD_SECONDS);
    assert_err!(env.review_hold_as(&owner.pubkey(), 1, 50, &new_reviewer), NotReviewer);
    assert!(env.change_roles(&admin, None));
    let policy = env.hold_state();
    assert_eq!((policy.collector, policy.reviewer), (new_collector.pubkey(), new_reviewer.pubkey()));
    assert_eq!(policy.pending_roles_at, 0);
    // The receipt collected under the old keys is reviewed by the new reviewer,
    // and the old collector can't collect any more.
    assert_err!(env.review_hold(&owner.pubkey(), 1, 50), NotReviewer);
    assert!(env.review_hold_as(&owner.pubkey(), 1, 50, &new_reviewer));
    env.airdrop_dividend(&account, 5);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotCollector);
    // A proposal can be withdrawn.
    assert!(env.change_roles(&admin, roles));
    assert!(env.change_roles(&admin, Some((Pubkey::default(), Pubkey::default()))));
    assert_err!(env.change_roles(&admin, None), NoPendingParams);
}

#[test]
fn timeout_is_refundable_by_anyone_even_after_approval() {
    let (mut env, owner, account) = held(100);
    assert_err!(
        env.settle_hold(&owner.pubkey(), 0, &collector(), false),
        RefundNotAllowed
    );
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    env.warp(REFUND_SECONDS - HOLD_SECONDS);
    assert_err!(
        env.settle_hold(&owner.pubkey(), 0, &collector(), true),
        CollectionExpired
    );
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), false));
    assert_eq!(token_balance(&env.svm, &account), 100);
    // An expiry is the service's lapse, not the holder's choice: it stays
    // enrolled, with what came back under its baseline.
    assert!(env.consent_state(&owner.pubkey()).enabled);
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 100);
}

#[test]
fn refunds_cannot_be_redirected_and_work_after_source_account_closes() {
    let (mut env, owner, account) = held(100);
    let close = spl_token_2022::instruction::close_account(
        &env.inst.dividend_program,
        &account,
        &owner.pubkey(),
        &owner.pubkey(),
        &[],
    )
    .unwrap();
    assert!(send(&mut env.svm, &[close], &owner, &[&owner]));
    let mut wrong = env.settle_accounts(&owner.pubkey(), 0, &owner.pubkey());
    wrong.refund_account = env.dividend_vault();
    assert!(!env.settle_with(wrong, &owner, false));
    assert!(env.settle_hold(&owner.pubkey(), 0, &owner, false));
    assert_eq!(token_balance(&env.svm, &account), 100);
}

#[test]
fn goal_blocks_release_and_allows_permissionless_refund() {
    let (mut env, owner, account) = held(100);
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    let vault = env.coin_vault();
    env.set_balance(&vault, CONTRIBUTION_CAP);
    assert_err!(env.settle_hold(&owner.pubkey(), 0, &collector(), true), Completed);
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), false));
    assert!(env.config_state().milestone_reached);
    assert_eq!(token_balance(&env.svm, &account), 100);
}
