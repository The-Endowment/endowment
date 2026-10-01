//! The goal: once the coin vault holds `contribution_cap`, bought or sent
//! directly, contributions stop for good. Adapted from PR #2 (RoyEriksen).
use super::*;

fn goal_env() -> Env {
    pool_env_with(0, |params| {
        params.params.activate_bps = 0;
        params.params.deactivate_bps = 0;
        params.params.min_stake_bps = 0;
    })
}

#[test]
fn the_vault_balance_reaches_the_goal_without_a_buyback() {
    let mut env = goal_env();
    let (owner, account) = env.registered_landlord(0);
    let coin_vault = env.coin_vault();
    // The last base unit matters: below the goal, sweeps still run.
    env.set_balance(&coin_vault, CONTRIBUTION_CAP - 1);
    env.set_balance(&account, 100 * UNIT);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 0);
    assert!(!env.config_state().milestone_reached);

    // Coin sent directly completes the goal, with nothing bought.
    env.set_balance(&coin_vault, CONTRIBUTION_CAP);
    env.set_balance(&account, 100 * UNIT);
    let vault_before = token_balance(&env.svm, &env.dividend_vault());
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), vault_before);
    assert!(env.config_state().milestone_reached);
    assert_eq!(env.config_state().total_coin_bought, 0);
    let (late, _) = env.new_landlord(0);
    assert_err!(env.register(&late), Completed);

    // For good: a lower balance later and a parameter change don't undo it.
    env.set_balance(&coin_vault, CONTRIBUTION_CAP - 1);
    env.change_params(|params| params.tip_bps = params.tip_bps.saturating_sub(1));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT);
    assert!(!env.config_state().active);
}

#[test]
fn another_wallets_coin_cannot_complete_the_goal() {
    let mut env = goal_env();
    let (owner, account) = env.registered_landlord(0);
    let foreign = env.inst.coin_account(&owner.pubkey());
    env.set_balance(&foreign, CONTRIBUTION_CAP);
    let mut accounts = env.sweep_accounts(&owner.pubkey(), &account);
    accounts.coin_vault = foreign;
    assert_err!(env.sweep_with(accounts), WrongPool);
    assert!(!env.config_state().milestone_reached);
}

#[test]
fn the_goal_is_recorded_even_while_paused() {
    let mut env = goal_env();
    let (owner, account) = env.registered_landlord(0);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    env.set_balance(&env.coin_vault(), CONTRIBUTION_CAP);
    env.set_balance(&account, 100 * UNIT);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT);
    assert!(env.config_state().milestone_reached);
}

#[test]
fn no_count_switches_sweeps_back_on_after_the_goal() {
    let (mut env, owners) = counted_env();
    // Landlords count from their second count.
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(env.config_state().active);
    let account = env.inst.dividend_account(&owners[0].pubkey());
    env.set_balance(&env.coin_vault(), CONTRIBUTION_CAP);
    assert!(env.sweep(&owners[0].pubkey(), &account));
    assert!(env.config_state().milestone_reached && !env.config_state().active);
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(!env.config_state().active, "a count after the goal must not switch sweeps back on");
}
