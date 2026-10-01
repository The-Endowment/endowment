use crate::{
    constants::*,
    error::EndowmentError,
    events::RewardCollected,
    health::{ensure_tradeable, TradeAccounts},
    instructions::sweep::*,
    reports::{ReporterPolicy, RewardReport},
    state::Config,
};
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TransferChecked};

#[derive(Accounts)]
pub struct CollectReward<'info> {
    pub sweep: Sweep<'info>,
    #[account(seeds = [REPORTER_SEED, sweep.config.key().as_ref()], bump = reporter_policy.bump,
        constraint = reporter_policy.config == sweep.config.key() @ EndowmentError::NotReporter,
        constraint = !reporter_policy.disabled && reporter_policy.reporter == reporter.key() @ EndowmentError::NotReporter)]
    pub reporter_policy: Box<Account<'info, ReporterPolicy>>,
    pub reporter: Signer<'info>,
}

pub fn handle_collect_reward(ctx: Context<CollectReward>, report: RewardReport) -> Result<()> {
    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let a = &mut ctx.accounts.sweep;
    let config_key = a.config.key();
    // Persist a direct-donation completion latch without debiting the holder.
    if a.config.record_completion(config_key, a.coin_vault.amount) {
        return Ok(());
    }
    require!(!a.config.is_paused(now), EndowmentError::Paused);
    require!(!a.config.retired, EndowmentError::Retired);
    require!(a.config.active, EndowmentError::NotActive);
    require!(a.config.sweeps_on(now), EndowmentError::CountStale);
    report.validate(&a.config, &a.landlord, &ctx.accounts.reporter_policy, now)?;
    require!(
        a.dividend_account.delegate == Some(a.authority.key()).into(),
        EndowmentError::NotDelegated
    );
    require!(
        a.dividend_account.amount == report.expected_source_balance,
        EndowmentError::SourceBalanceChanged
    );
    let vault_before = a.dividend_vault.amount;
    require!(
        report.amount <= a.dividend_account.amount
            && report.amount <= a.dividend_account.delegated_amount
            && report.amount <= a.config.vault_cap().saturating_sub(vault_before),
        EndowmentError::CollectionLimit
    );
    ensure_tradeable(
        &a.config,
        &TradeAccounts {
            dividend_mint: &a.dividend_mint.to_account_info(),
            coin_mint: &a.coin_mint.to_account_info(),
            pool_state: &a.pool_state.to_account_info(),
            amm_config: &a.amm_config.to_account_info(),
            pool_dividend_vault: &a.pool_dividend_vault.to_account_info(),
            pool_coin_vault: &a.pool_coin_vault.to_account_info(),
            vaults: [
                &a.dividend_vault.to_account_info(),
                &a.coin_vault.to_account_info(),
            ],
        },
        clock.epoch,
    )?;
    let bump = [a.config.authority_bump];
    let seeds = Config::authority_seeds(&config_key, &bump);
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            a.dividend_token_program.key(),
            TransferChecked {
                from: a.dividend_account.to_account_info(),
                mint: a.dividend_mint.to_account_info(),
                to: a.dividend_vault.to_account_info(),
                authority: a.authority.to_account_info(),
            },
            &[&seeds],
        ),
        report.amount,
        a.dividend_mint.decimals,
    )?;
    a.dividend_vault.reload()?;
    let received = a
        .dividend_vault
        .amount
        .checked_sub(vault_before)
        .ok_or(EndowmentError::Overflow)?;
    let landlord = &mut a.landlord;
    landlord.last_report_nonce = report.nonce;
    landlord.total_contributed = landlord
        .total_contributed
        .checked_add(report.amount)
        .ok_or(EndowmentError::Overflow)?;
    landlord.last_sweep_at = now;
    a.config.total_swept = a
        .config
        .total_swept
        .checked_add(received)
        .ok_or(EndowmentError::Overflow)?;
    a.config.last_sweep_at = now;
    emit!(RewardCollected {
        config: config_key,
        owner: landlord.owner,
        consent_id: report.consent_id,
        nonce: report.nonce,
        collection_epoch: report.collection_epoch,
        reporter_epoch: report.reporter_epoch,
        amount: report.amount,
        received,
        evidence_hash: report.evidence_hash,
    });
    Ok(())
}
