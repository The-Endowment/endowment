//! Tracked collection is an owner opt-in, with two independent attestations.
//! These tests exercise the on-chain ceilings. Payout classification is tested
//! separately by the collector/reviewer replay tests in the worker repository.
use super::collection_hold::reviewer;
use super::*;

fn permit_pda(env: &Env, owner: &Pubkey) -> Pubkey {
    pda(&[PERMIT_SEED, env.config().as_ref(), owner.as_ref()])
}
fn enable_ix(env: &Env, owner: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::EnableTrackedCollection {}.data(),
        endowment::accounts::EnableCollection {
            owner: *owner,
            config: env.config(),
            policy: policy_pda(&env.config()),
            consent: consent_pda(&env.config(), owner),
            landlord: landlord_pda(&env.config(), owner),
            dividend_account: env.inst.dividend_account(owner),
        }
        .to_account_metas(None),
    )
}
fn tracked(start: u64) -> (Env, Keypair, Pubkey) {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(start);
    let ixs = [
        env.resync_ix(&owner.pubkey()),
        enable_ix(&env, &owner.pubkey()),
    ];
    assert!(send(&mut env.svm, &ixs, &owner, &[&owner]));
    env.seed_eligible_allowance(&env.config(), &landlord_pda(&env.config(), &owner.pubkey()));
    (env, owner, account)
}
fn report(env: &Env, owner: &Pubkey, amount: u64) -> CollectionReport {
    CollectionReport {
        consent_epoch: env.consent_state(owner).epoch,
        expected_balance: token_balance(&env.svm, &env.inst.dividend_account(owner)),
        amount,
        valid_until: env.now() + 45,
        evidence_hash: [3; 32],
    }
}
fn authorize(
    env: &mut Env,
    owner: &Pubkey,
    nonce: u64,
    r: CollectionReport,
    signer: &Keypair,
) -> bool {
    let ix = Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::AuthorizeCollection { nonce, report: r }.data(),
        endowment::accounts::AuthorizeCollection {
            reviewer: signer.pubkey(),
            owner: *owner,
            config: env.config(),
            policy: policy_pda(&env.config()),
            consent: consent_pda(&env.config(), owner),
            landlord: landlord_pda(&env.config(), owner),
            dividend_account: env.inst.dividend_account(owner),
            authority: env.authority(),
            permit: permit_pda(env, owner),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
    );
    send(&mut env.svm, &[ix], signer, &[signer])
}
fn collect(
    env: &mut Env,
    owner: &Pubkey,
    nonce: u64,
    r: CollectionReport,
    permit: Option<Pubkey>,
) -> bool {
    let mut accounts = env
        .sweep_accounts(owner, &env.inst.dividend_account(owner))
        .to_account_metas(None);
    if let Some(key) = permit {
        accounts.push(AccountMeta::new_readonly(key, false));
    }
    let ix = Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::Sweep { nonce, report: r }.data(),
        accounts,
    );
    let signer = collector();
    send(&mut env.svm, &[ix], &signer, &[&signer])
}
fn approved_collect(env: &mut Env, owner: &Pubkey, amount: u64) {
    let nonce = env.consent_state(owner).next_nonce;
    let r = report(env, owner, amount);
    assert!(authorize(env, owner, nonce, r, &reviewer()));
    let r = report(env, owner, amount);
    let key = permit_pda(env, owner);
    assert!(collect(env, owner, nonce, r, Some(key)));
}

#[test]
fn spending_below_enrollment_does_not_block_new_attested_rewards() {
    let (mut env, owner, account) = tracked(100);
    // Spend 70 of the original holding, buy 10, then earn 20: balance 60.
    // Both independent workers attest only the 20, not the purchase.
    env.set_balance(&account, 30);
    env.airdrop_dividend(&account, 10);
    env.airdrop_dividend(&account, 20);
    approved_collect(&mut env, &owner.pubkey(), 20);
    assert_eq!(token_balance(&env.svm, &account), 40);
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 40);
    assert_eq!(token_balance(&env.svm, &env.pending_vault()), 20);
    // A second interval still works below the old 100 floor.
    env.airdrop_dividend(&account, 7);
    approved_collect(&mut env, &owner.pubkey(), 7);
    assert_eq!(token_balance(&env.svm, &account), 40);
    assert_eq!(env.consent_state(&owner.pubkey()).next_nonce, 2);
}

#[test]
fn legacy_holders_keep_the_floor_and_only_owner_can_change_mode() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(100);
    assert!(!env.landlord_state(&owner.pubkey()).tracked_rewards());
    env.set_balance(&account, 60);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 60);
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 100);
    // An already-enabled consent cannot be switched by the new instruction.
    let ix = enable_ix(&env, &owner.pubkey());
    assert_err!(
        send(&mut env.svm, &[ix], &owner, &[&owner]),
        InvalidCollection
    );
    assert!(env.disable_hold(&owner));
    // Replacing the signer does not let a third party target this landlord.
    let mut ix = enable_ix(&env, &owner.pubkey());
    ix.accounts[0].pubkey = reviewer().pubkey();
    let signer = reviewer();
    assert!(!send(&mut env.svm, &[ix], &signer, &[&signer]));
    assert!(!env.landlord_state(&owner.pubkey()).tracked_rewards());
    let ix = enable_ix(&env, &owner.pubkey());
    assert!(send(&mut env.svm, &[ix], &owner, &[&owner]));
    assert!(env.landlord_state(&owner.pubkey()).tracked_rewards());
    // Owners can also explicitly return to legacy mode.
    assert!(env.disable_hold(&owner));
    assert!(env.enable_hold(&owner));
    assert!(!env.landlord_state(&owner.pubkey()).tracked_rewards());
}

#[test]
fn tracked_collector_cannot_bypass_or_exceed_independent_permit() {
    let (mut env, owner, account) = tracked(100);
    let key = permit_pda(&env, &owner.pubkey());
    let r = report(&env, &owner.pubkey(), 20);
    assert_err!(
        collect(&mut env, &owner.pubkey(), 0, r, None),
        InvalidCollection
    );
    let r = report(&env, &owner.pubkey(), 20);
    assert_err!(
        authorize(&mut env, &owner.pubkey(), 0, r, &collector()),
        NotReviewer
    );
    let r = report(&env, &owner.pubkey(), 20);
    assert!(authorize(&mut env, &owner.pubkey(), 0, r, &reviewer()));
    let r = report(&env, &owner.pubkey(), 21);
    assert_err!(
        collect(&mut env, &owner.pubkey(), 0, r, Some(key)),
        InvalidCollection
    );
    let r = report(&env, &owner.pubkey(), 20);
    assert!(collect(&mut env, &owner.pubkey(), 0, r, Some(key)));
    assert_eq!(token_balance(&env.svm, &account), 80);
    // The next nonce cannot reuse this permit, even after a purchase restores balance.
    env.airdrop_dividend(&account, 20);
    let r = report(&env, &owner.pubkey(), 20);
    assert_err!(
        collect(&mut env, &owner.pubkey(), 1, r, Some(key)),
        InvalidCollection
    );
}

#[test]
fn permit_is_bound_to_balance_epoch_deadline_and_count() {
    for change in 0..5 {
        let (mut env, owner, account) = tracked(100);
        let key = permit_pda(&env, &owner.pubkey());
        let r = report(&env, &owner.pubkey(), 20);
        assert!(authorize(&mut env, &owner.pubkey(), 0, r, &reviewer()));
        match change {
            0 => env.airdrop_dividend(&account, 1),
            1 => {
                env.warp(46);
            }
            2 => {
                // Successful new owner consent invalidates every old permit.
                assert!(env.disable_hold(&owner));
                let ix = enable_ix(&env, &owner.pubkey());
                assert!(send(&mut env.svm, &[ix], &owner, &[&owner]));
            }
            _ => {
                // Count/refresher boundaries cannot be bypassed by a grant.
                let mut acc = env.svm.get_account(&env.config()).unwrap();
                let mut c = env.config_state();
                if change == 3 {
                    c.last_count_at -= 1;
                } else {
                    c.refresher_epoch += 1;
                }
                c.try_serialize(&mut &mut acc.data[..]).unwrap();
                env.svm.set_account(env.config(), acc).unwrap();
            }
        }
        let r = report(&env, &owner.pubkey(), 20);
        assert_err!(
            collect(&mut env, &owner.pubkey(), 0, r, Some(key)),
            InvalidCollection
        );
        assert_eq!(token_balance(&env.svm, &env.pending_vault()), 0);
    }
}

#[test]
fn tracked_mode_preserves_refund_and_stop_rights() {
    let (mut env, owner, account) = tracked(100);
    env.set_balance(&account, 50);
    approved_collect(&mut env, &owner.pubkey(), 20);
    assert_eq!(token_balance(&env.svm, &account), 30);
    assert!(env.disable_hold(&owner));
    assert!(env.settle_hold(&owner.pubkey(), 0, &owner, false));
    assert_eq!(token_balance(&env.svm, &account), 50);
    let r = report(&env, &owner.pubkey(), 20);
    assert_err!(
        authorize(&mut env, &owner.pubkey(), 1, r, &reviewer()),
        CollectionConsentRequired
    );
}

#[test]
fn reviewer_cannot_authorize_more_than_current_onchain_reward_credit() {
    let (mut env, owner, _) = tracked(100);
    let key = landlord_pda(&env.config(), &owner.pubkey());
    let mut acc = env.svm.get_account(&key).unwrap();
    let mut landlord = env.landlord_state(&owner.pubkey());
    landlord.allowance = 7;
    landlord.try_serialize(&mut &mut acc.data[..]).unwrap();
    env.svm.set_account(key, acc).unwrap();
    let r = report(&env, &owner.pubkey(), 8);
    assert_err!(
        authorize(&mut env, &owner.pubkey(), 0, r, &reviewer()),
        InvalidCollection
    );
    approved_collect(&mut env, &owner.pubkey(), 7);
}

#[test]
fn malformed_or_unbounded_permits_cannot_be_created() {
    for change in 0..7 {
        let (mut env, owner, _) = tracked(100);
        let mut r = report(&env, &owner.pubkey(), 20);
        let mut nonce = 0;
        match change {
            0 => r.valid_until = env.now(),
            1 => r.valid_until = env.now() + PERMIT_SECONDS + 1,
            2 => r.evidence_hash = [0; 32],
            3 => r.consent_epoch += 1,
            4 => r.expected_balance += 1,
            5 => nonce = 1,
            _ => r.amount = 0,
        }
        assert_err!(
            authorize(&mut env, &owner.pubkey(), nonce, r, &reviewer()),
            InvalidCollection
        );
        assert!(env
            .svm
            .get_account(&permit_pda(&env, &owner.pubkey()))
            .is_none());
    }
}

#[test]
fn pre_funding_a_permit_does_not_block_reviewer_initialization() {
    let (mut env, owner, _) = tracked(100);
    let key = permit_pda(&env, &owner.pubkey());
    env.svm.airdrop(&key, 1).unwrap();
    let r = report(&env, &owner.pubkey(), 20);
    assert!(authorize(&mut env, &owner.pubkey(), 0, r, &reviewer()));
    let r = report(&env, &owner.pubkey(), 20);
    assert!(collect(&mut env, &owner.pubkey(), 0, r, Some(key)));
}

#[test]
fn changed_reviewer_pause_or_wrong_permit_cannot_use_an_old_authorization() {
    for change in 0..3 {
        let (mut env, owner, _) = tracked(100);
        let r = report(&env, &owner.pubkey(), 20);
        assert!(authorize(&mut env, &owner.pubkey(), 0, r, &reviewer()));
        let mut permit = permit_pda(&env, &owner.pubkey());
        match change {
            0 => {
                let key = policy_pda(&env.config());
                let mut acc = env.svm.get_account(&key).unwrap();
                let mut policy = env.hold_state();
                policy.reviewer = owner.pubkey();
                policy.try_serialize(&mut &mut acc.data[..]).unwrap();
                env.svm.set_account(key, acc).unwrap();
            }
            1 => {
                let guardian = env.guardian.insecure_clone();
                let admin = env.admin();
                assert!(env.pause(&guardian));
                assert!(env.unpause(&admin));
            }
            _ => permit = policy_pda(&env.config()),
        }
        let r = report(&env, &owner.pubkey(), 20);
        assert_err!(
            collect(&mut env, &owner.pubkey(), 0, r, Some(permit)),
            InvalidCollection
        );
    }
}

#[test]
fn a_partial_tracked_collection_consumes_the_permit_and_refund_stays_recoverable() {
    let (mut env, owner, account) = tracked(100);
    let key = permit_pda(&env, &owner.pubkey());
    let r = report(&env, &owner.pubkey(), 20);
    assert!(authorize(&mut env, &owner.pubkey(), 0, r, &reviewer()));
    let r = report(&env, &owner.pubkey(), 7);
    assert!(collect(&mut env, &owner.pubkey(), 0, r, Some(key)));
    assert_eq!(token_balance(&env.svm, &account), 93);
    let r = report(&env, &owner.pubkey(), 13);
    assert_err!(
        collect(&mut env, &owner.pubkey(), 1, r, Some(key)),
        InvalidCollection
    );
    // A post-signing same-balance race can still be rejected by post-review.
    // The reviewer can refund immediately, without waiting for release review.
    assert!(env.settle_hold(&owner.pubkey(), 0, &reviewer(), false));
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert!(env.consent_state(&owner.pubkey()).enabled);
    assert_eq!(env.landlord_state(&owner.pubkey()).total_contributed, 0);
}

#[test]
fn a_zero_amount_tracked_collection_consumes_its_nonce_without_a_receipt() {
    let (mut env, owner, account) = tracked(100);
    let r = report(&env, &owner.pubkey(), 20);
    assert!(authorize(&mut env, &owner.pubkey(), 0, r, &reviewer()));
    let key = permit_pda(&env, &owner.pubkey());
    let r = report(&env, &owner.pubkey(), 0);
    assert!(collect(&mut env, &owner.pubkey(), 0, r, Some(key)));
    assert_eq!(env.consent_state(&owner.pubkey()).next_nonce, 1);
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert!(env
        .svm
        .get_account(&receipt_pda(&env.config(), &owner.pubkey(), 0))
        .is_none_or(|a| a.lamports == 0));
    let r = report(&env, &owner.pubkey(), 20);
    assert_err!(
        collect(&mut env, &owner.pubkey(), 1, r, Some(key)),
        InvalidCollection
    );
}

#[test]
fn export_tracked_abi_fixture() {
    let Ok(path) = std::env::var("TRACKED_FIXTURE_PATH") else {
        return;
    };
    use base64::Engine;
    let (mut env, owner, account) = tracked(100);
    env.set_balance(&account, 60);
    let r = report(&env, &owner.pubkey(), 20);
    let data = endowment::instruction::AuthorizeCollection {
        nonce: 0,
        report: r,
    }
    .data();
    let r = report(&env, &owner.pubkey(), 20);
    assert!(authorize(&mut env, &owner.pubkey(), 0, r, &reviewer()));
    let key = permit_pda(&env, &owner.pubkey());
    let permit = env.svm.get_account(&key).unwrap();
    let a = endowment::accounts::AuthorizeCollection {
        reviewer: reviewer().pubkey(),
        owner: owner.pubkey(),
        config: env.config(),
        policy: policy_pda(&env.config()),
        consent: consent_pda(&env.config(), &owner.pubkey()),
        landlord: landlord_pda(&env.config(), &owner.pubkey()),
        dividend_account: account,
        authority: env.authority(),
        permit: key,
        system_program: system_program::ID,
    };
    std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
        "program": endowment::id().to_string(), "config": env.config().to_string(),
        "owner": owner.pubkey().to_string(), "reviewer": reviewer().pubkey().to_string(),
        "coinMint": env.inst.coin_mint.to_string(), "dividendMint": env.inst.dividend_mint.to_string(),
        "coinTokenProgram": env.inst.coin_program.to_string(), "dividendTokenProgram": env.inst.dividend_program.to_string(),
        "enableData": enable_ix(&env, &owner.pubkey()).data, "authorizeData": data,
        "authorizeAccounts": a.to_account_metas(None).iter().map(|m| serde_json::json!({
            "address": m.pubkey.to_string(), "writable": m.is_writable, "signer": m.is_signer,
        })).collect::<Vec<_>>(),
        "permitAddress": key.to_string(), "permitData": base64::engine::general_purpose::STANDARD.encode(permit.data),
    })).unwrap()).unwrap();
}
