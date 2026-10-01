use super::collection_hold::{held, reviewer};
use super::*;

#[test]
fn six_holder_count_and_refresh_fit_the_companion_keeper_compute_budget() {
    let mut env = Env::new();
    env.create();
    let owners: Vec<Pubkey> = (0..6)
        .map(|_| env.registered_holder(0, 100_000 * UNIT).0.pubkey())
        .collect();
    let caller = refresher();
    let refresh = env.refresh_ix_by(env.config(), &owners, &caller.pubkey());
    assert!(send(
        &mut env.svm,
        &[compute_limit_ix(185_000), refresh],
        &caller,
        &[&caller]
    ));
    assert!(last_cu() <= 185_000);
    assert!(env.begin());
    let count = env.count_landlords_ix(env.config(), &owners);
    assert!(send(
        &mut env.svm,
        &[compute_limit_ix(185_000), count],
        &caller,
        &[&caller]
    ));
    assert!(last_cu() <= 185_000);
}

fn report(env: &Env, owner: &Pubkey, account: &Pubkey) -> CollectionReport {
    CollectionReport {
        consent_epoch: env.consent_state(owner).epoch,
        expected_balance: token_balance(&env.svm, account),
        amount: 100,
        valid_until: env.now() + REPORT_SECONDS,
        evidence_hash: [7; 32],
    }
}
fn submit_report(
    env: &mut Env,
    owner: &Pubkey,
    account: &Pubkey,
    report: CollectionReport,
    nonce: u64,
    signer: &Keypair,
) -> bool {
    let mut accounts = env.sweep_accounts(owner, account);
    accounts.collector = signer.pubkey();
    accounts.receipt = receipt_pda(&env.config(), owner, nonce);
    let ix = Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::Sweep { nonce, report }.data(),
        accounts.to_account_metas(None),
    );
    send(&mut env.svm, &[ix], signer, &[signer])
}
#[test]
fn stale_or_forged_collection_reports_cannot_move_tokens() {
    let (mut env, owner, account) = held(100);
    env.airdrop_dividend(&account, 100);
    let valid = report(&env, &owner.pubkey(), &account);
    let mut wrong = valid.clone();
    wrong.expected_balance += 1;
    assert_err!(
        submit_report(&mut env, &owner.pubkey(), &account, wrong, 1, &collector()),
        InvalidCollection
    );
    let mut wrong = valid.clone();
    wrong.consent_epoch += 1;
    assert_err!(
        submit_report(&mut env, &owner.pubkey(), &account, wrong, 1, &collector()),
        InvalidCollection
    );
    let mut wrong = valid.clone();
    wrong.valid_until += 1;
    assert_err!(
        submit_report(&mut env, &owner.pubkey(), &account, wrong, 1, &collector()),
        InvalidCollection
    );
    let mut wrong = valid.clone();
    wrong.valid_until = env.now() - 1;
    assert_err!(
        submit_report(&mut env, &owner.pubkey(), &account, wrong, 1, &collector()),
        InvalidCollection
    );
    assert_err!(
        submit_report(&mut env, &owner.pubkey(), &account, valid.clone(), 2, &collector()),
        InvalidCollection
    );
    assert_err!(
        submit_report(&mut env, &owner.pubkey(), &account, valid.clone(), 1, &reviewer()),
        NotCollector
    );
    assert!(submit_report(
        &mut env,
        &owner.pubkey(),
        &account,
        valid.clone(),
        1,
        &collector()
    ));
    assert!(!submit_report(
        &mut env,
        &owner.pubkey(),
        &account,
        valid,
        1,
        &collector()
    ));
    assert_eq!(env.hold_state().pending, 200);
}
#[test]
fn legacy_sweep_data_and_alternative_pending_vault_are_rejected() {
    let (mut env, owner, account) = held(100);
    env.airdrop_dividend(&account, 100);
    let mut accounts = env.sweep_accounts(&owner.pubkey(), &account);
    let data = endowment::instruction::Sweep {
        nonce: 1,
        report: report(&env, &owner.pubkey(), &account),
    }
    .data();
    let legacy = Instruction::new_with_bytes(endowment::id(), &data[..8], accounts.to_account_metas(None));
    assert!(!send(&mut env.svm, &[legacy], &collector(), &[&collector()]));
    let (_, other_vault) = env.new_landlord(0);
    accounts.pending_vault = other_vault;
    assert_err!(env.sweep_with(accounts), InvalidCollection);
    assert_eq!(token_balance(&env.svm, &account), 100);
}
#[test]
fn holder_can_reclaim_an_approved_receipt_and_old_nonces_never_reopen() {
    let (mut env, owner, account) = held(100);
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    assert!(env.settle_hold(&owner.pubkey(), 0, &owner, false));
    assert!(env.enable_hold(&owner));
    let valid = report(&env, &owner.pubkey(), &account);
    assert_err!(
        submit_report(&mut env, &owner.pubkey(), &account, valid, 0, &collector()),
        InvalidCollection
    );
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert!(!env.settle_hold(&owner.pubkey(), 0, &collector(), true));
}
#[test]
fn refunding_one_receipt_cancels_other_pending_releases_even_after_reenrollment() {
    let (mut env, owner, account) = held(100);
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    assert!(env.review_hold(&owner.pubkey(), 1, 50));
    assert!(env.settle_hold(&owner.pubkey(), 0, &owner, false));
    assert_err!(
        env.settle_hold(&owner.pubkey(), 1, &collector(), true),
        CollectionConsentRequired
    );
    assert!(env.enable_hold(&owner));
    assert_err!(
        env.settle_hold(&owner.pubkey(), 1, &collector(), true),
        CollectionConsentRequired
    );
    assert!(env.settle_hold(&owner.pubkey(), 1, &collector(), false));
    assert!(!env.consent_state(&owner.pubkey()).enabled);
    assert_eq!(token_balance(&env.svm, &account), 150);
}
#[test]
fn unsolicited_pending_deposits_do_not_enlarge_any_receipt() {
    let (mut env, owner, account) = held(100);
    let vault = env.pending_vault();
    env.airdrop_dividend(&vault, 500);
    env.warp(HOLD_SECONDS);
    assert_err!(env.review_hold(&owner.pubkey(), 0, 600), InvalidCollection);
    assert!(env.settle_hold(&owner.pubkey(), 0, &owner, false));
    assert_eq!(token_balance(&env.svm, &vault), 500);
    assert_eq!(env.hold_state().pending, 0);
    assert_eq!(token_balance(&env.svm, &account), 100);
}
#[test]
fn refund_still_works_after_retirement_and_renunciation() {
    let (mut env, owner, account) = held(100);
    let admin = env.admin();
    assert!(env.admin_call(&admin, endowment::instruction::Retire {}.data()));
    env.warp(PARAM_TIMELOCK_SECONDS);
    assert!(env.admin_call(&admin, endowment::instruction::Retire {}.data()));
    assert!(env.admin_call(&admin, endowment::instruction::RenounceAdmin {}.data()));
    assert!(env.settle_hold(&owner.pubkey(), 0, &collector(), false));
    assert_eq!(token_balance(&env.svm, &account), 100);
}
#[test]
fn export_cross_language_hold_fixture() {
    let Ok(path) = std::env::var("HOLD_FIXTURE_PATH") else {
        return;
    };
    use base64::Engine;
    let (env, owner, account) = held(1000);
    let mut keys = env
        .sweep_accounts(&owner.pubkey(), &account)
        .to_account_metas(None)
        .iter()
        .map(|m| m.pubkey)
        .collect::<Vec<_>>();
    keys.push(receipt_pda(&env.config(), &owner.pubkey(), 0));
    keys.push(env.inst.coin_account(&owner.pubkey()));
    let records=keys.into_iter().filter_map(|key|env.svm.get_account(&key).map(|a|(key.to_string(),serde_json::json!({"owner":a.owner.to_string(),"data":[base64::engine::general_purpose::STANDARD.encode(a.data),"base64"]})))).collect::<serde_json::Map<_,_>>();
    let r = report(&env, &owner.pubkey(), &account);
    let accounts = env.sweep_accounts(&owner.pubkey(), &account);
    let data = endowment::instruction::Sweep { nonce: 1, report: r }.data();
    std::fs::write(path,serde_json::to_vec_pretty(&serde_json::json!({
        "program":endowment::id().to_string(),"config":env.config().to_string(),"owner":owner.pubkey().to_string(),
        "collector":collector().pubkey().to_string(),"reviewer":reviewer().pubkey().to_string(),
        "coinMint":env.inst.coin_mint.to_string(),"dividendMint":env.inst.dividend_mint.to_string(),
        "coinTokenProgram":env.inst.coin_program.to_string(),"dividendTokenProgram":env.inst.dividend_program.to_string(),
        "pool":env.inst.pool.to_string(),"records":records,"sweepData":data,
        "sweepAccounts":accounts.to_account_metas(None).iter().map(|m|serde_json::json!({"address":m.pubkey.to_string(),"writable":m.is_writable,"signer":m.is_signer})).collect::<Vec<_>>()
    })).unwrap()).unwrap();
}

#[test]
fn accepted_zero_amount_report_consumes_nonce_without_leaving_a_receipt() {
    let (mut env, owner, account) = held(100);
    assert_eq!(env.consent_state(&owner.pubkey()).next_nonce, 1);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(env.consent_state(&owner.pubkey()).next_nonce, 2);
    assert!(env
        .svm
        .get_account(&receipt_pda(&env.config(), &owner.pubkey(), 1))
        .is_none_or(|a| a.lamports == 0));
    assert_eq!(env.hold_state().pending, 100);
}

#[test]
fn disabled_collection_consent_counts_zero_even_if_delegation_remains() {
    let mut env = Env::new();
    env.create();
    let (owner, _) = env.registered_holder(0, 400_000_000 * UNIT);
    let (other, _) = env.new_landlord(0);
    env.mint_coin(&other.pubkey(), 600_000_000 * UNIT);
    assert!(env.count());
    env.warp(DAY);
    assert!(env.count());
    assert!(env.config_state().active);
    let ix = Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::DisableCollection {}.data(),
        endowment::accounts::DisableCollection {
            owner: owner.pubkey(),
            consent: consent_pda(&env.config(), &owner.pubkey()),
        }
        .to_account_metas(None),
    );
    assert!(send(&mut env.svm, &[ix], &owner, &[&owner]));
    env.warp(DAY);
    assert!(env.count());
    assert_eq!(env.config_state().last_committed, 0);
    assert!(!env.config_state().active);
}
