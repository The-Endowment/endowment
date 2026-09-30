//! Completion follows authenticated direct holdings and permanently stops collection.
use super::*;

#[test]
fn audit_direct_vault_target_stops_sweeps_without_a_buyback() {
    let mut env = pool_env_with(0, |params| {
        params.params.activate_bps = 0;
        params.params.deactivate_bps = 0;
        params.params.min_stake_bps = 0;
    });
    let (owner, account) = env.registered_landlord(0);
    let coin_vault = env.coin_vault();
    // The final token base unit matters; holdings below the target still collect.
    env.set_balance(&coin_vault, CONTRIBUTION_CAP - 1);
    env.set_balance(&account, 100 * UNIT);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 0);
    assert!(!env.config_state().milestone_reached);

    // A direct donation completes funding despite zero cumulative purchases.
    env.set_balance(&coin_vault, CONTRIBUTION_CAP);
    env.set_balance(&account, 100 * UNIT);
    let vault_before = token_balance(&env.svm, &env.dividend_vault());
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), vault_before);
    assert!(env.config_state().milestone_reached);
    assert_eq!(env.config_state().total_coin_bought, 0);
    let (late_holder, _) = env.new_landlord(0);
    assert_err!(env.register(&late_holder), Completed);

    // Completion is permanent even if a future balance observation is lower
    // and an admin reapplies zero activation thresholds.
    env.set_balance(&coin_vault, CONTRIBUTION_CAP - 1);
    env.change_params(|params| params.tip_bps = params.tip_bps.saturating_sub(1));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT);
    assert!(!env.config_state().active);
}

#[test]
fn audit_another_wallets_vault_cannot_complete_the_campaign() {
    let mut env = pool_env_with(0, |params| {
        params.params.activate_bps = 0;
        params.params.deactivate_bps = 0;
        params.params.min_stake_bps = 0;
    });
    let (owner, account) = env.registered_landlord(0);
    let foreign_vault = env.inst.coin_account(&owner.pubkey());
    env.set_balance(&foreign_vault, CONTRIBUTION_CAP);
    let mut accounts = env.sweep_accounts(&owner.pubkey(), &account);
    accounts.coin_vault = foreign_vault;
    assert_err!(env.sweep_with(accounts), WrongPool);
    assert!(!env.config_state().milestone_reached);
}

#[test]
fn audit_completion_is_recorded_even_while_sweeping_is_paused() {
    let mut env = pool_env_with(0, |params| {
        params.params.activate_bps = 0;
        params.params.deactivate_bps = 0;
        params.params.min_stake_bps = 0;
    });
    let (owner, account) = env.registered_landlord(0);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    env.set_balance(&env.coin_vault(), CONTRIBUTION_CAP);
    env.set_balance(&account, 100 * UNIT);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT);
    assert!(env.config_state().milestone_reached);
}

