use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::{
    constants::*,
    error::EndowmentError,
    events::LandlordRemoved,
    state::{Config, Landlord},
    transfer::read_token_account,
};

/// Permissionless: removes a landlord that no longer qualifies, so the daily
/// count doesn't have to keep reading it. A landlord qualifies while its dividend
/// account still delegates to the endowment and it holds at least the minimum
/// stake. The rent goes back to the landlord's own wallet.
#[derive(Accounts)]
pub struct PruneLandlord<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.coin_mint.as_ref(), config.creator.as_ref()],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: this endowment's authority PDA; compared with the delegate.
    #[account(seeds = [AUTHORITY_SEED, config.key().as_ref()], bump = config.authority_bump)]
    pub authority: UncheckedAccount<'info>,
    #[account(
        mut,
        close = owner,
        seeds = [LANDLORD_SEED, config.key().as_ref(), landlord.owner.as_ref()],
        bump = landlord.bump,
        has_one = config,
        has_one = owner,
    )]
    pub landlord: Box<Account<'info, Landlord>>,
    /// CHECK: the landlord's wallet; receives the rent. Checked by `has_one`.
    #[account(mut)]
    pub owner: UncheckedAccount<'info>,
    #[account(address = config.coin_mint)]
    pub coin_mint: Box<InterfaceAccount<'info, Mint>>,
    /// CHECK: the landlord's registered dividend account (may be closed); read raw.
    #[account(address = landlord.dividend_account)]
    pub dividend_account: UncheckedAccount<'info>,
    /// CHECK: the landlord's registered coin account (may be closed); read raw.
    #[account(address = landlord.coin_account)]
    pub coin_account: UncheckedAccount<'info>,
}

pub fn handle_prune_landlord(ctx: Context<PruneLandlord>) -> Result<()> {
    let authority = ctx.accounts.authority.key();
    let owner = ctx.accounts.landlord.owner;
    // An account reassigned to someone else (legacy SPL Token allows it) no
    // longer holds or delegates anything for this landlord.
    let delegated = read_token_account(&ctx.accounts.dividend_account)?
        .map(|t| t.owner == owner && t.delegates_to(&authority))
        .unwrap_or(false);
    let held = read_token_account(&ctx.accounts.coin_account)?
        .map(|t| if t.owner == owner { t.amount } else { 0 })
        .unwrap_or(0);
    let config = &mut ctx.accounts.config;
    let staked = held >= config.min_stake(ctx.accounts.coin_mint.supply);
    require!(!delegated || !staked, EndowmentError::NotPrunable);

    let landlord = &ctx.accounts.landlord;
    config.remove_from_round(landlord);
    config.landlord_count = config.landlord_count.saturating_sub(1);
    emit!(LandlordRemoved { config: config.key(), owner: landlord.owner });
    Ok(())
}
