use anchor_lang::{prelude::*, AccountsClose};
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    constants::*,
    error::EndowmentError,
    events::{LandlordRegistered, LandlordRemoved},
    state::{Config, Landlord, Roster, RosterEntry},
};

/// Sent in the same transaction as the landlord's unlimited
/// `Approve(dividend_account → authority)` for this endowment.
///
/// The roster holds at most `MAX_LANDLORDS`. When it's full, a newcomer can
/// take the place of the landlord with the smallest recorded stake if the
/// newcomer holds more: pass that landlord's record and wallet as
/// `evict_landlord` / `evict_owner`. The evicted landlord's rent goes back to them.
#[derive(Accounts)]
pub struct RegisterLandlord<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: this endowment's authority PDA; address check only.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        init,
        payer = owner,
        space = 8 + Landlord::INIT_SPACE,
        seeds = [LANDLORD_SEED, config.key().as_ref(), owner.key().as_ref()],
        bump
    )]
    pub landlord: Box<Account<'info, Landlord>>,
    #[account(mut, seeds = [ROSTER_SEED, config.key().as_ref()], bump = config.roster_bump)]
    pub roster: Box<Account<'info, Roster>>,

    #[account(address = config.dividend_mint)]
    pub dividend_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        associated_token::mint = dividend_mint,
        associated_token::authority = owner,
        associated_token::token_program = dividend_token_program,
        constraint = dividend_account.delegate == Some(authority.key()).into()
            && dividend_account.delegated_amount >= MIN_DELEGATION
            @ EndowmentError::NotDelegated,
    )]
    pub dividend_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    /// Counted toward activation in each commitment count.
    #[account(
        associated_token::mint = coin_mint,
        associated_token::authority = owner,
        associated_token::token_program = coin_token_program,
    )]
    pub coin_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub dividend_token_program: Interface<'info, TokenInterface>,
    pub coin_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,

    /// Only when the roster is full: the landlord being replaced.
    #[account(mut, has_one = config)]
    pub evict_landlord: Option<Box<Account<'info, Landlord>>>,
    /// CHECK: only when the roster is full: the replaced landlord's wallet, which gets its rent back.
    #[account(mut)]
    pub evict_owner: Option<UncheckedAccount<'info>>,
}

pub fn handle_register_landlord(ctx: Context<RegisterLandlord>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let config = &ctx.accounts.config;
    let config_key = config.key();
    require!(!config.retired, EndowmentError::Retired);
    require!(!config.is_paused(now), EndowmentError::Paused);

    let coin_held = ctx.accounts.coin_account.amount;
    require!(coin_held >= config.min_stake(ctx.accounts.coin_mint.supply), EndowmentError::StakeTooSmall);

    let owner = ctx.accounts.owner.key();
    let roster = &mut ctx.accounts.roster;
    if roster.entries.len() >= MAX_LANDLORDS {
        // Full: replace the smallest recorded stake, if the newcomer holds more.
        let smallest = roster.smallest().ok_or(EndowmentError::RosterFull)?;
        let (Some(evicted), Some(evict_owner)) =
            (ctx.accounts.evict_landlord.as_ref(), ctx.accounts.evict_owner.as_ref())
        else {
            return err!(EndowmentError::RosterFull);
        };
        let entry = roster.entries[smallest];
        require_keys_eq!(evicted.owner, entry.owner, EndowmentError::InvalidEviction);
        require_keys_eq!(evict_owner.key(), entry.owner, EndowmentError::InvalidEviction);
        let canonical = Pubkey::create_program_address(
            &[LANDLORD_SEED, config_key.as_ref(), entry.owner.as_ref(), &[evicted.bump]],
            ctx.program_id,
        )
        .map_err(|_| error!(EndowmentError::InvalidEviction))?;
        require_keys_eq!(evicted.key(), canonical, EndowmentError::InvalidEviction);
        require!(coin_held > entry.snapshot, EndowmentError::InvalidEviction);

        roster.entries.remove(smallest);
        evicted.close(evict_owner.to_account_info())?;
        emit!(LandlordRemoved { config: config_key, owner: entry.owner, evicted_by: Some(owner) });
    }

    let baseline = ctx.accounts.dividend_account.amount;
    roster.entries.push(RosterEntry {
        owner,
        coin_account: ctx.accounts.coin_account.key(),
        dividend_account: ctx.accounts.dividend_account.key(),
        snapshot: coin_held,
        // Counted only from the second count after registering.
        snapshot_valid: false,
    });

    ctx.accounts.landlord.set_inner(Landlord {
        version: LANDLORD_VERSION,
        config: config_key,
        owner,
        dividend_account: ctx.accounts.dividend_account.key(),
        coin_account: ctx.accounts.coin_account.key(),
        baseline,
        total_contributed: 0,
        registered_at: now,
        last_sweep_at: 0,
        bump: ctx.bumps.landlord,
        reserved: [0; 64],
    });

    emit!(LandlordRegistered { config: config_key, owner, baseline, coin_held });
    Ok(())
}
