//! Regressions from the independent review of the hold layer at e2cb97e.
use super::collection_hold::{held, reviewer};
use super::*;

const MEMO: Pubkey = endowment::constants::MEMO_PROGRAM_ID;

fn post_total(env: &mut Env, total: u64) {
    let signer = refresher();
    let ix = Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::PostRewardTotal { total }.data(),
        endowment::accounts::PostRewardTotal { refresher: signer.pubkey(), config: env.config(), coin_mint: env.inst.coin_mint }
            .to_account_metas(None),
    );
    assert!(send(&mut env.svm, &[ix], &signer, &[&signer]));
}

/// Counted landlords with the allowance on and sweeps running.
fn allowance_env() -> (Env, Vec<Keypair>) {
    let (mut env, owners) = counted_env();
    env.change_params(|p| {
        p.allowance_margin_bps = ALLOWANCE_MARGIN_BPS;
        p.max_rewards_per_day = 1_000_000 * UNIT;
    });
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(env.config_state().active);
    (env, owners)
}

/// The landlord's own token account now refuses transfers that carry no memo.
fn require_memos(env: &mut Env, owner: &Keypair, account: &Pubkey) {
    use spl_token_2022::extension::{memo_transfer::instruction::enable_required_transfer_memos, ExtensionType};
    let program = env.inst.dividend_program;
    let ixs = [
        spl_token_2022::instruction::reallocate(&program, account, &owner.pubkey(), &owner.pubkey(), &[], &[ExtensionType::MemoTransfer])
            .unwrap(),
        enable_required_transfer_memos(&program, account, &owner.pubkey(), &[]).unwrap(),
    ];
    assert!(send(&mut env.svm, &ixs, owner, &[owner]));
}

/// A settlement that also passes the Memo program, as the endowment's services do.
fn settle_with_memo(env: &mut Env, owner: &Pubkey, nonce: u64, caller: &Keypair, release: bool) -> bool {
    let data = if release {
        endowment::instruction::ReleaseCollection {}.data()
    } else {
        endowment::instruction::RefundCollection {}.data()
    };
    let mut metas = env.settle_accounts(owner, nonce, &caller.pubkey()).to_account_metas(None);
    metas.push(AccountMeta::new_readonly(MEMO, false));
    let ix = Instruction::new_with_bytes(endowment::id(), &data, metas);
    send(&mut env.svm, &[ix], caller, &[caller])
}

#[test]
fn a_memo_requiring_account_cant_make_its_receipts_unsettleable() {
    let (mut env, owner, account) = held(100);
    require_memos(&mut env, &owner, &account);
    // Without the Memo program the token program refuses the refund,
    assert!(!env.settle_hold(&owner.pubkey(), 0, &reviewer(), false));
    assert_eq!(env.hold_state().pending, 100);
    // with it, the reviewer's refund lands, and so does a partial release.
    assert!(settle_with_memo(&mut env, &owner.pubkey(), 0, &reviewer(), false));
    assert_eq!((env.hold_state().pending, token_balance(&env.svm, &account)), (0, 100));

    let (mut env, owner, account) = held(100);
    require_memos(&mut env, &owner, &account);
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 60));
    assert!(settle_with_memo(&mut env, &owner.pubkey(), 0, &collector(), true));
    assert_eq!((token_balance(&env.svm, &env.dividend_vault()), token_balance(&env.svm, &account)), (60, 40));
}

#[test]
fn only_the_memo_program_is_accepted_as_the_extra_account() {
    let (mut env, owner, _) = held(100);
    let caller = reviewer();
    let mut metas = env.settle_accounts(&owner.pubkey(), 0, &caller.pubkey()).to_account_metas(None);
    metas.push(AccountMeta::new_readonly(system_program::ID, false));
    let ix = Instruction::new_with_bytes(endowment::id(), &endowment::instruction::RefundCollection {}.data(), metas);
    assert_err!(send(&mut env.svm, &[ix], &caller, &[&caller]), InvalidCollection);
}

#[test]
fn one_post_after_a_gap_credits_two_days_not_the_whole_gap() {
    let (mut env, owners) = allowance_env();
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    post_total(&mut env, 0);
    // The post service is out for five days while counts continue; holders
    // were paid 5,000 PUMP in that time. The landlord holds a tenth.
    for _ in 0..5 {
        env.warp(DAY);
        assert!(env.count());
    }
    post_total(&mut env, 5_000 * UNIT);
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    // Two days' share of the increase is 200 at most (the gap ran a little over
    // five days, so slightly less); the whole gap would have been 500.
    let swept = 1_000 * UNIT - token_balance(&env.svm, &account);
    assert!(swept <= 200 * UNIT && swept > 150 * UNIT, "swept {swept}");
}

#[test]
fn rewards_paid_during_a_pause_are_not_credited_after_it() {
    let (mut env, owners) = allowance_env();
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    post_total(&mut env, 0);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    env.warp(2 * DAY);
    let admin = env.admin();
    assert!(env.unpause(&admin));
    assert!(env.count());
    let index = env.config_state().reward_index;
    post_total(&mut env, 2_000 * UNIT);
    assert_eq!(env.config_state().reward_index, index);
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT);
    // The next full day is credited as usual.
    env.warp(DAY);
    assert!(env.count());
    post_total(&mut env, 3_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 900 * UNIT);
}

#[test]
fn collection_cant_be_switched_on_twice_and_switching_on_costs_a_round() {
    let (mut env, owners) = allowance_env();
    let owner = owners[0].pubkey();
    assert_err!(env.enable_hold(&owners[0]), InvalidCollection);
    // Off and on again: the landlord's coin has to be read and counted afresh.
    assert!(env.disable_hold(&owners[0]));
    assert!(env.enable_hold(&owners[0]));
    let landlord = env.landlord_state(&owner);
    assert_eq!((landlord.snapshot_valid, landlord.attestations), (false, 0));
    env.warp(DAY);
    assert!(env.count());
    assert_eq!(env.landlord_state(&owner).counted_amount, 0);
}

#[test]
fn coin_sold_after_the_count_stops_earning_at_the_next_read() {
    let (mut env, owners) = allowance_env();
    let owner = owners[0].pubkey();
    let account = env.inst.dividend_account(&owner);
    post_total(&mut env, 0);
    let counted = env.landlord_state(&owner).counted_amount;
    let (other, _) = env.new_landlord(0);
    let ix = env.coin_transfer_ix(&owner, &other.pubkey(), counted);
    assert!(send(&mut env.svm, &[ix], &owners[0], &[&owners[0]]));
    assert!(env.refresh(&[owner]));
    env.warp(22 * 3600);
    post_total(&mut env, 1_000 * UNIT);
    env.airdrop_dividend(&account, 1_000 * UNIT);
    assert!(env.sweep(&owner, &account));
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT);
}

#[test]
fn a_pruned_landlords_receipt_is_refunded_not_released() {
    let (mut env, owner, account) = held(100);
    env.revoke(&owner);
    assert!(env.prune(&owner.pubkey()));
    env.warp(HOLD_SECONDS);
    assert!(env.review_hold(&owner.pubkey(), 0, 100));
    assert_err!(env.settle_hold(&owner.pubkey(), 0, &collector(), true), CollectionConsentRequired);
    let stranger = env.funded();
    assert!(env.settle_hold(&owner.pubkey(), 0, &stranger, false));
    assert_eq!(token_balance(&env.svm, &account), 100);
}

#[test]
fn renouncing_needs_a_rewards_ceiling_that_caps_something() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    env.change_params(|p| {
        p.allowance_margin_bps = ALLOWANCE_MARGIN_BPS;
        p.max_rewards_per_day = u64::MAX;
    });
    assert_err!(env.renounce(&admin), InvalidAllowance);
    env.change_params(|p| p.max_rewards_per_day = 10 * MAX_BUY_PER_DAY);
    assert!(env.renounce(&admin));
}

#[test]
fn a_role_change_cant_make_a_key_the_reviewer_of_what_it_collected() {
    let (mut env, _, _) = held(100);
    let admin = env.admin();
    let fresh = env.funded();
    assert_err!(env.change_roles(&admin, Some((fresh.pubkey(), collector().pubkey()))), InvalidCollectionPolicy);
    assert_err!(env.change_roles(&admin, Some((reviewer().pubkey(), fresh.pubkey()))), InvalidCollectionPolicy);
}
