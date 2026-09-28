use {
    anchor_lang::{
        prelude::{Clock, Pubkey},
        solana_program::{instruction::Instruction, program_pack::Pack, system_program},
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_spl::{
        associated_token::{
            get_associated_token_address_with_program_id as ata,
            spl_associated_token_account::instruction::create_associated_token_account_idempotent,
            ID as ATA_PROGRAM,
        },
        token_2022::{spl_token_2022, ID as TOKEN_2022},
    },
    endowment::{
        constants::{AUTHORITY_SEED, CONFIG_SEED, LANDLORD_SEED, MAX_PAUSE_SECONDS},
        state::{Config, Landlord},
    },
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    spl_token_2022::state::{Account as TokenAccount, Mint},
};

const DECIMALS: u8 = 6;
const PUMP: u64 = 1_000_000; // one PUMP in base units
const MAX_BUY_PER_TX: u64 = 5_000 * PUMP;
const MAX_BUY_PER_DAY: u64 = 20_000 * PUMP;
const MAX_PRICE_IMPACT_BPS: u16 = 100;

struct Env {
    svm: LiteSVM,
    deployer: Keypair,
    guardian: Keypair,
    mint_authority: Keypair,
    pump_mint: Pubkey,
    penis_mint: Pubkey,
    config: Pubkey,
    authority: Pubkey,
    pump_vault: Pubkey,
    pool: Pubkey,
}

/// Mainnet accounts for the Raydium CPMM PENIS/PUMP pool, fetched into
/// `tests/fixtures` (public on-chain data).
mod fixtures {
    use super::Pubkey;
    use base64::Engine;
    use std::str::FromStr;

    pub const POOL: &str = "AXTq4JHNYHSnooqjoDmtL9WW5eEgnkkMSWq76Kznidnz";
    pub const AMM_CONFIG: &str = "CRRS5ieQmBrZjWhcj99JuGrT5tyuWDaGAXLXLFjbAtjQ";
    pub const POOL_PUMP_VAULT: &str = "HEmGXak4vSj9Dkikm3H82fQyhYUhzcZ7ub5TVuzYv9FC";
    pub const POOL_PENIS_VAULT: &str = "D8h2adEhs9CR6Q5kRGEH3csGcsHxPDtpBwD1X4SbkugY";
    pub const OBSERVATION: &str = "CijsijpVmMdKZdZLKnwbtDpcL6zE6o5smqfujExWRRGF";
    pub const PUMP_MINT: &str = "pumpCmXqMfrsAkQ5r49WcJnRayYRqmXz6ae8H7H9Dfn";
    pub const PENIS_MINT: &str = "JE3HT7SbCgXDQWV6xp3oiiAisDzq4HyZ8wyEVBDCs45Z";

    pub fn key(s: &str) -> Pubkey {
        Pubkey::from_str(s).unwrap()
    }

    pub fn cpmm_program() -> Pubkey {
        key("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C")
    }

    pub fn load(name: &str) -> (Pubkey, solana_account::Account) {
        let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = base64::engine::general_purpose::STANDARD.decode(json["data"].as_str().unwrap()).unwrap();
        let account = solana_account::Account {
            lamports: json["lamports"].as_u64().unwrap(),
            data,
            owner: key(json["owner"].as_str().unwrap()),
            executable: false,
            rent_epoch: 0,
        };
        (key(json["pubkey"].as_str().unwrap()), account)
    }
}

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &endowment::id()).0
}

fn send(svm: &mut LiteSVM, ixs: &[Instruction], payer: &Keypair, signers: &[&Keypair]) -> bool {
    let msg = Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &svm.latest_blockhash());
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    let result = svm.send_transaction(tx);
    if let Err(failed) = &result {
        // Captured by the test harness; shown only when a test fails.
        eprintln!("tx failed: {:?}\n{}", failed.err, failed.meta.logs.join("\n"));
    }
    svm.expire_blockhash();
    result.is_ok()
}

fn token_balance(svm: &LiteSVM, account: &Pubkey) -> u64 {
    TokenAccount::unpack(&svm.get_account(account).unwrap().data[..TokenAccount::LEN])
        .unwrap()
        .amount
}

fn landlord_state(svm: &LiteSVM, owner: &Pubkey) -> Landlord {
    let account = svm.get_account(&pda(&[LANDLORD_SEED, owner.as_ref()])).unwrap();
    Landlord::try_deserialize(&mut account.data.as_slice()).unwrap()
}

fn create_mint(svm: &mut LiteSVM, authority: &Pubkey) -> Pubkey {
    let mint = Keypair::new().pubkey();
    let mut data = vec![0u8; Mint::LEN];
    Mint::pack(
        Mint {
            mint_authority: Some(*authority).into(),
            supply: 0,
            decimals: DECIMALS,
            is_initialized: true,
            freeze_authority: None.into(),
        },
        &mut data,
    )
    .unwrap();
    svm.set_account(
        mint,
        solana_account::Account {
            lamports: svm.minimum_balance_for_rent_exemption(Mint::LEN),
            data,
            owner: TOKEN_2022,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    mint
}

impl Env {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let bytes = include_bytes!(concat!(env!("CARGO_TARGET_TMPDIR"), "/../deploy/endowment.so"));
        svm.add_program(endowment::id(), bytes).unwrap();

        let deployer = Keypair::new();
        let guardian = Keypair::new();
        let mint_authority = Keypair::new();
        for kp in [&deployer, &guardian, &mint_authority] {
            svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
        }

        // LiteSVM deploys with no upgrade authority; make the deployer the upgrade authority.
        let program_data = Self::program_data();
        let mut account = svm.get_account(&program_data).unwrap();
        account.data[12] = 1;
        account.data[13..45].copy_from_slice(deployer.pubkey().as_ref());
        svm.set_account(program_data, account).unwrap();

        let pump_mint = create_mint(&mut svm, &mint_authority.pubkey());
        let penis_mint = create_mint(&mut svm, &mint_authority.pubkey());
        let authority = pda(&[AUTHORITY_SEED]);

        Env {
            config: pda(&[CONFIG_SEED]),
            pump_vault: ata(&authority, &pump_mint, &TOKEN_2022),
            svm,
            deployer,
            guardian,
            mint_authority,
            pump_mint,
            penis_mint,
            authority,
            pool: Pubkey::new_unique(),
        }
    }

    /// An environment with the real Raydium CPMM program and the mainnet
    /// PENIS/PUMP pool, both mints and the pool's vaults loaded from fixtures.
    fn with_pool() -> Self {
        let mut env = Env::new();
        let cpmm = include_bytes!("fixtures/raydium_cp_swap.so");
        env.svm.add_program(fixtures::cpmm_program(), cpmm).unwrap();
        for name in ["pool_state", "amm_config", "token_0_vault", "token_1_vault", "token_0_mint", "token_1_mint", "observation_state"] {
            let (pubkey, account) = fixtures::load(name);
            env.svm.set_account(pubkey, account).unwrap();
        }
        env.pump_mint = fixtures::key(fixtures::PUMP_MINT);
        env.penis_mint = fixtures::key(fixtures::PENIS_MINT);
        env.pool = fixtures::key(fixtures::POOL);
        env.pump_vault = ata(&env.authority, &env.pump_mint, &TOKEN_2022);

        // Swaps need a clock after the pool opened and after its last observation.
        let mut clock: Clock = env.svm.get_sysvar();
        clock.unix_timestamp = 1_790_700_000;
        env.svm.set_sysvar(&clock);
        env
    }

    fn penis_vault(&self) -> Pubkey {
        ata(&self.authority, &self.penis_mint, &TOKEN_2022)
    }

    /// Writes a token account's balance directly (the real PUMP mint can't be minted).
    fn set_balance(&mut self, account: &Pubkey, amount: u64) {
        let mut acc = self.svm.get_account(account).unwrap();
        acc.data[64..72].copy_from_slice(&amount.to_le_bytes());
        self.svm.set_account(*account, acc).unwrap();
    }

    fn buyback_accounts(&self) -> endowment::accounts::Buyback {
        let cpmm = fixtures::cpmm_program();
        endowment::accounts::Buyback {
            config: self.config,
            authority: self.authority,
            pump_mint: self.pump_mint,
            penis_mint: self.penis_mint,
            pump_vault: self.pump_vault,
            penis_vault: self.penis_vault(),
            cpmm_program: cpmm,
            cpmm_authority: Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &cpmm).0,
            amm_config: fixtures::key(fixtures::AMM_CONFIG),
            pool_state: self.pool,
            pool_pump_vault: fixtures::key(fixtures::POOL_PUMP_VAULT),
            pool_penis_vault: fixtures::key(fixtures::POOL_PENIS_VAULT),
            observation_state: fixtures::key(fixtures::OBSERVATION),
            pump_token_program: TOKEN_2022,
            penis_token_program: TOKEN_2022,
        }
    }

    fn buyback_with(&mut self, accounts: endowment::accounts::Buyback, amount_in: u64, min_out: u64) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Buyback { amount_in, min_out }.data(),
            accounts.to_account_metas(None),
        );
        let cranker = self.funded();
        send(&mut self.svm, &[ix], &cranker, &[&cranker])
    }

    fn buyback(&mut self, amount_in: u64, min_out: u64) -> bool {
        let accounts = self.buyback_accounts();
        self.buyback_with(accounts, amount_in, min_out)
    }

    fn set_buyback_limits(&mut self, signer: &Keypair, per_tx: u64, per_day: u64, impact_bps: u16) -> bool {
        let ix = self.admin_ix(
            signer,
            endowment::instruction::SetBuybackLimits {
                max_buy_per_tx: per_tx,
                max_buy_per_day: per_day,
                max_price_impact_bps: impact_bps,
            }
            .data(),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn program_data() -> Pubkey {
        Pubkey::find_program_address(
            &[endowment::id().as_ref()],
            &anchor_lang::solana_program::bpf_loader_upgradeable::id(),
        )
        .0
    }

    fn initialize_ix(&self, payer: &Pubkey) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Initialize {
                admin: self.deployer.pubkey(),
                guardian: self.guardian.pubkey(),
                supply_target_bps: 2_000,
                pool: self.pool,
                max_buy_per_tx: MAX_BUY_PER_TX,
                max_buy_per_day: MAX_BUY_PER_DAY,
                max_price_impact_bps: MAX_PRICE_IMPACT_BPS,
            }
            .data(),
            endowment::accounts::Initialize {
                payer: *payer,
                config: self.config,
                authority: self.authority,
                pump_mint: self.pump_mint,
                penis_mint: self.penis_mint,
                pump_vault: self.pump_vault,
                penis_vault: ata(&self.authority, &self.penis_mint, &TOKEN_2022),
                program: endowment::id(),
                program_data: Self::program_data(),
                pump_token_program: TOKEN_2022,
                penis_token_program: TOKEN_2022,
                associated_token_program: ATA_PROGRAM,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    fn initialize(&mut self) {
        let ix = self.initialize_ix(&self.deployer.pubkey());
        let deployer = self.deployer.insecure_clone();
        assert!(send(&mut self.svm, &[ix], &deployer, &[&deployer]));
    }

    /// A landlord wallet holding `starting` PUMP in its ATA.
    fn new_landlord(&mut self, starting: u64) -> (Keypair, Pubkey) {
        let owner = Keypair::new();
        self.svm.airdrop(&owner.pubkey(), 1_000_000_000).unwrap();
        let account = ata(&owner.pubkey(), &self.pump_mint, &TOKEN_2022);
        let create = create_associated_token_account_idempotent(
            &owner.pubkey(),
            &owner.pubkey(),
            &self.pump_mint,
            &TOKEN_2022,
        );
        assert!(send(&mut self.svm, &[create], &owner, &[&owner]));
        self.airdrop_pump(&account, starting);
        (owner, account)
    }

    /// Simulates a PUMP dividend drop landing in a holder's account.
    fn airdrop_pump(&mut self, account: &Pubkey, amount: u64) {
        if amount == 0 {
            return;
        }
        let ix = spl_token_2022::instruction::mint_to(
            &TOKEN_2022,
            &self.pump_mint,
            account,
            &self.mint_authority.pubkey(),
            &[],
            amount,
        )
        .unwrap();
        let auth = self.mint_authority.insecure_clone();
        assert!(send(&mut self.svm, &[ix], &auth, &[&auth]));
    }

    fn approve_ix(&self, owner: &Pubkey, account: &Pubkey) -> Instruction {
        spl_token_2022::instruction::approve(&TOKEN_2022, account, &self.authority, owner, &[], u64::MAX)
            .unwrap()
    }

    fn register_ix(&self, owner: &Pubkey, account: &Pubkey) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::RegisterLandlord {}.data(),
            endowment::accounts::RegisterLandlord {
                owner: *owner,
                config: self.config,
                authority: self.authority,
                landlord: pda(&[LANDLORD_SEED, owner.as_ref()]),
                pump_mint: self.pump_mint,
                pump_account: *account,
                pump_token_program: TOKEN_2022,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    fn sweep(&mut self, owner: &Pubkey, account: &Pubkey) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Sweep {}.data(),
            endowment::accounts::Sweep {
                config: self.config,
                authority: self.authority,
                landlord: pda(&[LANDLORD_SEED, owner.as_ref()]),
                pump_mint: self.pump_mint,
                pump_account: *account,
                pump_vault: self.pump_vault,
                pump_token_program: TOKEN_2022,
            }
            .to_account_metas(None),
        );
        // Anyone can crank a sweep.
        let cranker = Keypair::new();
        self.svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
        send(&mut self.svm, &[ix], &cranker, &[&cranker])
    }

    fn pause(&mut self, signer: &Keypair) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Pause {}.data(),
            endowment::accounts::Pause { guardian: signer.pubkey(), config: self.config }.to_account_metas(None),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn unpause(&mut self, signer: &Keypair) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Unpause {}.data(),
            endowment::accounts::Unpause { admin: signer.pubkey(), config: self.config }.to_account_metas(None),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn admin_ix(&self, signer: &Keypair, data: Vec<u8>) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &data,
            endowment::accounts::AdminOnly { admin: signer.pubkey(), config: self.config }.to_account_metas(None),
        )
    }

    fn set_guardian(&mut self, signer: &Keypair, new_guardian: Pubkey) -> bool {
        let ix = self.admin_ix(signer, endowment::instruction::SetGuardian { new_guardian }.data());
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn propose_admin(&mut self, signer: &Keypair, new_admin: Pubkey) -> bool {
        let ix = self.admin_ix(signer, endowment::instruction::ProposeAdmin { new_admin }.data());
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn accept_admin(&mut self, signer: &Keypair) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::AcceptAdmin {}.data(),
            endowment::accounts::AcceptAdmin { new_admin: signer.pubkey(), config: self.config }
                .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn config_state(&self) -> Config {
        let account = self.svm.get_account(&self.config).unwrap();
        Config::try_deserialize(&mut account.data.as_slice()).unwrap()
    }

    fn funded(&mut self) -> Keypair {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 1_000_000_000).unwrap();
        kp
    }

    fn warp(&mut self, seconds: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp += seconds;
        self.svm.set_sysvar(&clock);
    }
}

#[test]
fn only_upgrade_authority_can_initialize() {
    let mut env = Env::new();
    let stranger = Keypair::new();
    env.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    let ix = env.initialize_ix(&stranger.pubkey());
    assert!(!send(&mut env.svm, &[ix], &stranger, &[&stranger]));

    env.initialize();
    let account = env.svm.get_account(&env.config).unwrap();
    let config = Config::try_deserialize(&mut account.data.as_slice()).unwrap();
    assert_eq!(config.guardian, env.guardian.pubkey());
    assert_eq!(config.supply_target_bps, 2_000);
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 0);
}

#[test]
fn register_requires_delegation() {
    let mut env = Env::new();
    env.initialize();
    let (owner, account) = env.new_landlord(100);

    let register = env.register_ix(&owner.pubkey(), &account);
    assert!(!send(&mut env.svm, &[register.clone()], &owner, &[&owner]));

    let approve = env.approve_ix(&owner.pubkey(), &account);
    assert!(send(&mut env.svm, &[approve, register], &owner, &[&owner]));
    assert_eq!(landlord_state(&env.svm, &owner.pubkey()).baseline, 100);
}

#[test]
fn sweeps_only_new_pump_and_tracks_contributions() {
    let mut env = Env::new();
    env.initialize();
    let (owner, account) = env.new_landlord(100);
    let ixs = [env.approve_ix(&owner.pubkey(), &account), env.register_ix(&owner.pubkey(), &account)];
    assert!(send(&mut env.svm, &ixs, &owner, &[&owner]));

    // A dividend drop lands; the sweep takes only the new PUMP.
    env.airdrop_pump(&account, 250);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 250);
    assert_eq!(landlord_state(&env.svm, &owner.pubkey()).total_contributed, 250);

    // Nothing new: a sweep is a harmless no-op.
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 250);

    // The landlord spends 60 of their own PUMP; the baseline follows them down.
    let burn = spl_token_2022::instruction::burn(&TOKEN_2022, &account, &env.pump_mint, &owner.pubkey(), &[], 60)
        .unwrap();
    assert!(send(&mut env.svm, &[burn], &owner, &[&owner]));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(landlord_state(&env.svm, &owner.pubkey()).baseline, 40);

    env.airdrop_pump(&account, 10);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 40);
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 260);
    let landlord = landlord_state(&env.svm, &owner.pubkey());
    assert_eq!((landlord.baseline, landlord.total_contributed), (40, 260));
}

#[test]
fn drop_landing_after_a_spend_is_left_with_the_landlord() {
    // The program can't see a balance's low point between sweeps, so a drop that
    // lands after the landlord spends below their baseline, but before the next
    // sweep, is treated as the landlord's own. It errs toward the landlord.
    let mut env = Env::new();
    env.initialize();
    let (owner, account) = env.new_landlord(100);
    let ixs = [env.approve_ix(&owner.pubkey(), &account), env.register_ix(&owner.pubkey(), &account)];
    assert!(send(&mut env.svm, &ixs, &owner, &[&owner]));

    let burn = spl_token_2022::instruction::burn(&TOKEN_2022, &account, &env.pump_mint, &owner.pubkey(), &[], 60)
        .unwrap();
    assert!(send(&mut env.svm, &[burn], &owner, &[&owner]));
    env.airdrop_pump(&account, 10);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 50);
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 0);
    assert_eq!(landlord_state(&env.svm, &owner.pubkey()).baseline, 50);
}

#[test]
fn revoking_delegation_stops_sweeps() {
    let mut env = Env::new();
    env.initialize();
    let (owner, account) = env.new_landlord(0);
    let ixs = [env.approve_ix(&owner.pubkey(), &account), env.register_ix(&owner.pubkey(), &account)];
    assert!(send(&mut env.svm, &ixs, &owner, &[&owner]));

    // Opting out uses the token program directly, not the endowment.
    let revoke = spl_token_2022::instruction::revoke(&TOKEN_2022, &account, &owner.pubkey(), &[]).unwrap();
    assert!(send(&mut env.svm, &[revoke], &owner, &[&owner]));

    env.airdrop_pump(&account, 500);
    assert!(!env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 500);
}

#[test]
fn guardian_pause_blocks_sweeps_and_expires() {
    let mut env = Env::new();
    env.initialize();
    let (owner, account) = env.new_landlord(0);
    let ixs = [env.approve_ix(&owner.pubkey(), &account), env.register_ix(&owner.pubkey(), &account)];
    assert!(send(&mut env.svm, &ixs, &owner, &[&owner]));
    env.airdrop_pump(&account, 50);

    let stranger = env.funded();
    assert!(!env.pause(&stranger));

    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    assert!(!env.sweep(&owner.pubkey(), &account));

    env.warp(MAX_PAUSE_SECONDS + 1);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 50);
}

#[test]
fn guardian_pauses_but_only_admin_unpauses_early() {
    let mut env = Env::new();
    env.initialize();
    let (owner, account) = env.new_landlord(0);
    let ixs = [env.approve_ix(&owner.pubkey(), &account), env.register_ix(&owner.pubkey(), &account)];
    assert!(send(&mut env.svm, &ixs, &owner, &[&owner]));

    let guardian = env.guardian.insecure_clone();
    let admin = env.deployer.insecure_clone();
    assert!(env.pause(&guardian));
    assert!(!env.unpause(&guardian));
    env.airdrop_pump(&account, 5);
    assert!(!env.sweep(&owner.pubkey(), &account));

    assert!(env.unpause(&admin));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 5);
}

#[test]
fn admin_rotates_the_guardian() {
    let mut env = Env::new();
    env.initialize();
    let old_guardian = env.guardian.insecure_clone();
    let admin = env.deployer.insecure_clone();
    let new_guardian = env.funded();

    let stranger = env.funded();
    assert!(!env.set_guardian(&stranger, stranger.pubkey()));
    assert!(!env.set_guardian(&old_guardian, new_guardian.pubkey()));

    assert!(env.set_guardian(&admin, new_guardian.pubkey()));
    assert_eq!(env.config_state().guardian, new_guardian.pubkey());
    assert!(!env.pause(&old_guardian));
    assert!(env.pause(&new_guardian));
}

#[test]
fn admin_handover_is_two_step() {
    let mut env = Env::new();
    env.initialize();
    let old_admin = env.deployer.insecure_clone();
    let new_admin = env.funded();
    let stranger = env.funded();

    // Only the admin can propose, and only the proposed key can accept.
    assert!(!env.propose_admin(&stranger, stranger.pubkey()));
    assert!(!env.accept_admin(&new_admin));
    assert!(env.propose_admin(&old_admin, new_admin.pubkey()));
    assert_eq!(env.config_state().admin, old_admin.pubkey());
    assert!(!env.accept_admin(&stranger));

    // Proposing the default key cancels.
    assert!(env.propose_admin(&old_admin, Pubkey::default()));
    assert!(!env.accept_admin(&new_admin));

    assert!(env.propose_admin(&old_admin, new_admin.pubkey()));
    assert!(env.accept_admin(&new_admin));
    let config = env.config_state();
    assert_eq!((config.admin, config.pending_admin), (new_admin.pubkey(), Pubkey::default()));

    // The old admin has lost its rights; the new one has them.
    assert!(!env.set_guardian(&old_admin, old_admin.pubkey()));
    assert!(env.set_guardian(&new_admin, new_admin.pubkey()));
}

// Buybacks run against the real Raydium CPMM program and mainnet pool state.

fn funded_pool_env(pump_in_vault: u64) -> Env {
    let mut env = Env::with_pool();
    env.initialize();
    let vault = env.pump_vault;
    env.set_balance(&vault, pump_in_vault);
    env
}

#[test]
fn buyback_swaps_pump_for_penis_into_the_vault() {
    let mut env = funded_pool_env(50_000 * PUMP);
    let pool_pump_before = token_balance(&env.svm, &fixtures::key(fixtures::POOL_PUMP_VAULT));

    assert!(env.buyback(1_000 * PUMP, 1));

    let received = token_balance(&env.svm, &env.penis_vault());
    assert!(received > 0);
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 49_000 * PUMP);
    assert_eq!(
        token_balance(&env.svm, &fixtures::key(fixtures::POOL_PUMP_VAULT)),
        pool_pump_before + 1_000 * PUMP
    );
    let config = env.config_state();
    assert_eq!(config.total_pump_spent, 1_000 * PUMP);
    assert_eq!(config.total_penis_bought, received);
    assert_eq!(config.bought_today, 1_000 * PUMP);
}

#[test]
fn buyback_enforces_per_trade_and_daily_caps() {
    let mut env = funded_pool_env(100_000 * PUMP);
    assert!(!env.buyback(MAX_BUY_PER_TX + 1, 1));

    for _ in 0..4 {
        assert!(env.buyback(MAX_BUY_PER_TX, 1));
    }
    assert!(!env.buyback(PUMP, 1));

    env.warp(24 * 60 * 60);
    assert!(env.buyback(MAX_BUY_PER_TX, 1));
    assert_eq!(env.config_state().bought_today, MAX_BUY_PER_TX);
}

#[test]
fn buyback_is_blocked_while_paused() {
    let mut env = funded_pool_env(10_000 * PUMP);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    assert!(!env.buyback(1_000 * PUMP, 1));
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 10_000 * PUMP);
}

#[test]
fn buyback_rejects_bad_fills() {
    let mut env = funded_pool_env(5_000_000 * PUMP);

    // The caller's own minimum is enforced.
    assert!(!env.buyback(1_000 * PUMP, u64::MAX));

    // A trade big enough to move the price well past 1% is refused, even with caps raised.
    let admin = env.deployer.insecure_clone();
    assert!(env.set_buyback_limits(&admin, 3_000_000 * PUMP, 3_000_000 * PUMP, MAX_PRICE_IMPACT_BPS));
    assert!(!env.buyback(2_000_000 * PUMP, 1));
    assert_eq!(token_balance(&env.svm, &env.pump_vault), 5_000_000 * PUMP);
    assert_eq!(token_balance(&env.svm, &env.penis_vault()), 0);
}

#[test]
fn buyback_rejects_accounts_from_other_pools() {
    let mut env = funded_pool_env(10_000 * PUMP);

    let mut wrong_pool = env.buyback_accounts();
    wrong_pool.pool_state = fixtures::key(fixtures::AMM_CONFIG);
    assert!(!env.buyback_with(wrong_pool, 1_000 * PUMP, 1));

    // The pool's two vaults swapped: PUMP would flow the wrong way.
    let mut swapped = env.buyback_accounts();
    std::mem::swap(&mut swapped.pool_pump_vault, &mut swapped.pool_penis_vault);
    assert!(!env.buyback_with(swapped, 1_000 * PUMP, 1));

    // Output can only land in the endowment's own $PENIS vault.
    let mut elsewhere = env.buyback_accounts();
    elsewhere.penis_vault = fixtures::key(fixtures::POOL_PENIS_VAULT);
    assert!(!env.buyback_with(elsewhere, 1_000 * PUMP, 1));

    assert!(env.buyback(1_000 * PUMP, 1));
}

#[test]
fn buyback_limits_are_admin_only_and_bounded() {
    let mut env = Env::new();
    env.initialize();
    let admin = env.deployer.insecure_clone();
    let stranger = env.funded();
    assert!(!env.set_buyback_limits(&stranger, PUMP, PUMP, 100));
    assert!(!env.set_buyback_limits(&admin, PUMP, PUMP, 301));
    assert!(!env.set_buyback_limits(&admin, 2 * PUMP, PUMP, 100));
    assert!(env.set_buyback_limits(&admin, PUMP, 10 * PUMP, 50));
    let config = env.config_state();
    assert_eq!((config.max_buy_per_tx, config.max_buy_per_day, config.max_price_impact_bps), (PUMP, 10 * PUMP, 50));
}
