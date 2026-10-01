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
fn partial_clearance_refunds_excess_and_disables_collection() {
    let (mut env, owner, account) = held(100);
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 60));
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), true));
    assert_eq!(token_balance(&env.svm, &account), 40);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 60);
    assert_eq!(env.hold_state().refunded, 40);
    assert_err!(env.sweep(&owner.pubkey(), &account), CollectionConsentRequired);
    assert!(env.enable_hold(&owner));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 40);
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
