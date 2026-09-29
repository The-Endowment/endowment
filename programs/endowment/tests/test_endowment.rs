use {
    anchor_lang::{
        prelude::{Clock, Pubkey},
        solana_program::{
            instruction::{AccountMeta, Instruction},
            program_pack::Pack,
            system_program,
        },
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_spl::{
        associated_token::{
            get_associated_token_address_with_program_id as ata,
            spl_associated_token_account::instruction::create_associated_token_account_idempotent,
            ID as ATA_PROGRAM,
        },
        token::ID as TOKEN,
        token_2022::{spl_token_2022, ID as TOKEN_2022},
    },
    endowment::{
        constants::{AUTHORITY_SEED, CONFIG_SEED, FLAGSHIP_CONFIG, LANDLORD_SEED, MAX_PAUSE_SECONDS},
        state::{Config, CreateParams, Landlord},
    },
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    spl_token_2022::state::{Account as TokenAccount, Mint},
};

const DECIMALS: u8 = 6;
const UNIT: u64 = 1_000_000; // one whole token in base units
const MAX_BUY_PER_TX: u64 = 5_000 * UNIT;
const MAX_BUY_PER_DAY: u64 = 20_000 * UNIT;
const MAX_PRICE_IMPACT_BPS: u16 = 100;
const CONTRIBUTION_CAP: u64 = 200_000_000 * UNIT;

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
    pub const LP_MINT: &str = "3T9NWNMJunF7dpCNX4vzyAuJo9WBUtJ2RXSk846TKbNQ";

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

fn config_pda(coin_mint: &Pubkey, creator: &Pubkey) -> Pubkey {
    pda(&[CONFIG_SEED, coin_mint.as_ref(), creator.as_ref()])
}

fn authority_pda(config: &Pubkey) -> Pubkey {
    pda(&[AUTHORITY_SEED, config.as_ref()])
}

fn landlord_pda(config: &Pubkey, owner: &Pubkey) -> Pubkey {
    pda(&[LANDLORD_SEED, config.as_ref(), owner.as_ref()])
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

fn create_mint(svm: &mut LiteSVM, authority: &Pubkey, program: &Pubkey) -> Pubkey {
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
            owner: *program,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    mint
}

/// A stand-in Raydium CPMM pool account holding `mints`, for tests that never
/// swap. Only the fields creation reads are filled in.
fn fake_pool(svm: &mut LiteSVM, mints: [Pubkey; 2], owner: Pubkey) -> Pubkey {
    let pool = Pubkey::new_unique();
    let mut data = vec![0u8; 637];
    data[..8].copy_from_slice(&[247, 237, 227, 245, 215, 195, 222, 70]);
    data[168..200].copy_from_slice(mints[0].as_ref());
    data[200..232].copy_from_slice(mints[1].as_ref());
    svm.set_account(
        pool,
        solana_account::Account {
            lamports: svm.minimum_balance_for_rent_exemption(data.len()),
            data,
            owner,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    pool
}

fn params(guardian: Pubkey, donation_bps: u16) -> CreateParams {
    CreateParams {
        admin: Pubkey::default(),
        guardian,
        max_buy_per_tx: MAX_BUY_PER_TX,
        max_buy_per_day: MAX_BUY_PER_DAY,
        max_price_impact_bps: MAX_PRICE_IMPACT_BPS,
        contribution_cap: CONTRIBUTION_CAP,
        activate_bps: 3_000,
        deactivate_bps: 2_500,
        buy_bps: 10_000,
        min_buy_interval_secs: 600,
        tip_bps: 25,
        donation_bps,
    }
}

/// One endowment: a coin, its dividend asset, its pool and its creator.
struct Inst {
    creator: Keypair,
    coin_mint: Pubkey,
    dividend_mint: Pubkey,
    coin_program: Pubkey,
    dividend_program: Pubkey,
    pool: Pubkey,
}

impl Clone for Inst {
    fn clone(&self) -> Self {
        Inst {
            creator: self.creator.insecure_clone(),
            coin_mint: self.coin_mint,
            dividend_mint: self.dividend_mint,
            coin_program: self.coin_program,
            dividend_program: self.dividend_program,
            pool: self.pool,
        }
    }
}

impl Inst {
    fn config(&self) -> Pubkey {
        config_pda(&self.coin_mint, &self.creator.pubkey())
    }
    fn authority(&self) -> Pubkey {
        authority_pda(&self.config())
    }
    fn dividend_vault(&self) -> Pubkey {
        ata(&self.authority(), &self.dividend_mint, &self.dividend_program)
    }
    fn coin_vault(&self) -> Pubkey {
        ata(&self.authority(), &self.coin_mint, &self.coin_program)
    }
    fn dividend_account(&self, owner: &Pubkey) -> Pubkey {
        ata(owner, &self.dividend_mint, &self.dividend_program)
    }
    fn coin_account(&self, owner: &Pubkey) -> Pubkey {
        ata(owner, &self.coin_mint, &self.coin_program)
    }
}

struct Env {
    svm: LiteSVM,
    guardian: Keypair,
    mint_authority: Keypair,
    inst: Inst,
}

impl Env {
    fn base() -> (LiteSVM, Keypair, Keypair, Keypair) {
        let mut svm = LiteSVM::new();
        let bytes = include_bytes!(concat!(env!("CARGO_TARGET_TMPDIR"), "/../deploy/endowment.so"));
        svm.add_program(endowment::id(), bytes).unwrap();
        let creator = Keypair::new();
        let guardian = Keypair::new();
        let mint_authority = Keypair::new();
        for kp in [&creator, &guardian, &mint_authority] {
            svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
        }
        (svm, creator, guardian, mint_authority)
    }

    /// Fresh Token-2022 mints for the coin and its dividend, and a stand-in pool.
    fn new() -> Self {
        Self::with_programs(TOKEN_2022, TOKEN_2022)
    }

    fn with_programs(coin_program: Pubkey, dividend_program: Pubkey) -> Self {
        let (mut svm, creator, guardian, mint_authority) = Self::base();
        let coin_mint = create_mint(&mut svm, &mint_authority.pubkey(), &coin_program);
        let dividend_mint = create_mint(&mut svm, &mint_authority.pubkey(), &dividend_program);
        let pool = fake_pool(&mut svm, [coin_mint, dividend_mint], fixtures::cpmm_program());
        Env {
            svm,
            guardian,
            mint_authority,
            inst: Inst { creator, coin_mint, dividend_mint, coin_program, dividend_program, pool },
        }
    }

    /// The real Raydium CPMM program and the mainnet PENIS/PUMP pool, both mints
    /// and the pool's vaults loaded from fixtures.
    fn with_pool() -> Self {
        let (mut svm, creator, guardian, mint_authority) = Self::base();
        let cpmm = include_bytes!("fixtures/raydium_cp_swap.so");
        svm.add_program(fixtures::cpmm_program(), cpmm).unwrap();
        for name in [
            "pool_state",
            "amm_config",
            "token_0_vault",
            "token_1_vault",
            "token_0_mint",
            "token_1_mint",
            "observation_state",
            "lp_mint",
        ] {
            let (pubkey, account) = fixtures::load(name);
            svm.set_account(pubkey, account).unwrap();
        }
        // Swaps need a clock after the pool opened and after its last observation.
        let mut clock: Clock = svm.get_sysvar();
        clock.unix_timestamp = 1_790_700_000;
        svm.set_sysvar(&clock);
        Env {
            svm,
            guardian,
            mint_authority,
            inst: Inst {
                creator,
                coin_mint: fixtures::key(fixtures::PENIS_MINT),
                dividend_mint: fixtures::key(fixtures::PUMP_MINT),
                coin_program: TOKEN_2022,
                dividend_program: TOKEN_2022,
                pool: fixtures::key(fixtures::POOL),
            },
        }
    }

    // Shortcuts to the current instance.
    fn config(&self) -> Pubkey {
        self.inst.config()
    }
    fn authority(&self) -> Pubkey {
        self.inst.authority()
    }
    fn dividend_vault(&self) -> Pubkey {
        self.inst.dividend_vault()
    }
    fn coin_vault(&self) -> Pubkey {
        self.inst.coin_vault()
    }
    fn admin(&self) -> Keypair {
        self.inst.creator.insecure_clone()
    }

    fn create_ix_for(&self, inst: &Inst, params: CreateParams) -> Instruction {
        let config = inst.config();
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::CreateEndowment { params }.data(),
            endowment::accounts::CreateEndowment {
                creator: inst.creator.pubkey(),
                config,
                authority: authority_pda(&config),
                coin_mint: inst.coin_mint,
                dividend_mint: inst.dividend_mint,
                dividend_vault: inst.dividend_vault(),
                coin_vault: inst.coin_vault(),
                pool_state: inst.pool,
                coin_token_program: inst.coin_program,
                dividend_token_program: inst.dividend_program,
                associated_token_program: ATA_PROGRAM,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    fn create_inst(&mut self, inst: &Inst, params: CreateParams) -> bool {
        let ix = self.create_ix_for(inst, params);
        let creator = inst.creator.insecure_clone();
        send(&mut self.svm, &[ix], &creator, &[&creator])
    }

    fn create_with(&mut self, params: CreateParams) -> bool {
        let inst = self.inst.clone();
        self.create_inst(&inst, params)
    }

    fn create(&mut self) {
        let guardian = self.guardian.pubkey();
        assert!(self.create_with(params(guardian, 0)));
    }

    /// Create with the activation threshold at 0, as for a founders-only test window.
    fn create_active(&mut self) {
        let mut p = params(self.guardian.pubkey(), 0);
        p.activate_bps = 0;
        p.deactivate_bps = 0;
        assert!(self.create_with(p));
        assert!(self.config_state().active);
    }

    /// Writes a token account's balance directly (the real mints can't be minted).
    fn set_balance(&mut self, account: &Pubkey, amount: u64) {
        let mut acc = self.svm.get_account(account).unwrap();
        acc.data[64..72].copy_from_slice(&amount.to_le_bytes());
        self.svm.set_account(*account, acc).unwrap();
    }

    fn lp_vault_for(authority: &Pubkey) -> Pubkey {
        ata(authority, &fixtures::key(fixtures::LP_MINT), &TOKEN)
    }

    fn lp_vault(&self) -> Pubkey {
        Self::lp_vault_for(&self.authority())
    }

    fn flagship_vault(&self) -> Pubkey {
        ata(&authority_pda(&FLAGSHIP_CONFIG), &self.inst.dividend_mint, &self.inst.dividend_program)
    }

    fn buyback_accounts_for(&self, inst: &Inst, caller: &Pubkey) -> endowment::accounts::Buyback {
        let cpmm = fixtures::cpmm_program();
        let authority = inst.authority();
        endowment::accounts::Buyback {
            config: inst.config(),
            authority,
            caller: *caller,
            caller_dividend_account: inst.dividend_account(caller),
            dividend_mint: inst.dividend_mint,
            coin_mint: inst.coin_mint,
            dividend_vault: inst.dividend_vault(),
            coin_vault: inst.coin_vault(),
            cpmm_program: cpmm,
            cpmm_authority: Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &cpmm).0,
            amm_config: fixtures::key(fixtures::AMM_CONFIG),
            pool_state: inst.pool,
            pool_dividend_vault: fixtures::key(fixtures::POOL_PUMP_VAULT),
            pool_coin_vault: fixtures::key(fixtures::POOL_PENIS_VAULT),
            observation_state: fixtures::key(fixtures::OBSERVATION),
            lp_mint: fixtures::key(fixtures::LP_MINT),
            lp_vault: Self::lp_vault_for(&authority),
            flagship_dividend_vault: ata(&authority_pda(&FLAGSHIP_CONFIG), &inst.dividend_mint, &inst.dividend_program),
            dividend_token_program: inst.dividend_program,
            coin_token_program: inst.coin_program,
            lp_token_program: TOKEN,
            token_2022_program: TOKEN_2022,
        }
    }

    fn buyback_accounts(&self, caller: &Pubkey) -> endowment::accounts::Buyback {
        self.buyback_accounts_for(&self.inst, caller)
    }

    /// Creates the (off-curve) LP account an instance's authority needs after close.
    fn create_lp_vault(&mut self, authority: &Pubkey) {
        let payer = self.funded();
        let create = create_associated_token_account_idempotent(
            &payer.pubkey(),
            authority,
            &fixtures::key(fixtures::LP_MINT),
            &TOKEN,
        );
        assert!(send(&mut self.svm, &[create], &payer, &[&payer]));
    }

    /// A funded keypair with a dividend token account, ready to crank buybacks.
    fn cranker(&mut self) -> Keypair {
        let kp = self.funded();
        let create = create_associated_token_account_idempotent(
            &kp.pubkey(),
            &kp.pubkey(),
            &self.inst.dividend_mint,
            &self.inst.dividend_program,
        );
        assert!(send(&mut self.svm, &[create], &kp, &[&kp]));
        kp
    }

    fn buyback_with(&mut self, caller: &Keypair, accounts: endowment::accounts::Buyback, min_out: u64) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Buyback { min_out }.data(),
            accounts.to_account_metas(None),
        );
        send(&mut self.svm, &[ix], caller, &[caller])
    }

    /// A buyback cranked by a fresh caller; returns the caller for tip checks.
    fn buyback(&mut self, min_out: u64) -> (bool, Keypair) {
        let caller = self.cranker();
        let accounts = self.buyback_accounts(&caller.pubkey());
        let ok = self.buyback_with(&caller, accounts, min_out);
        (ok, caller)
    }

    fn buy(&mut self) -> bool {
        self.buyback(1).0
    }

    fn admin_ix(&self, signer: &Keypair, data: Vec<u8>) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &data,
            endowment::accounts::AdminOnly { admin: signer.pubkey(), config: self.config() }.to_account_metas(None),
        )
    }

    fn admin_call(&mut self, signer: &Keypair, data: Vec<u8>) -> bool {
        let ix = self.admin_ix(signer, data);
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn set_buyback_limits(&mut self, signer: &Keypair, per_tx: u64, per_day: u64, impact_bps: u16) -> bool {
        self.admin_call(
            signer,
            endowment::instruction::SetBuybackLimits {
                max_buy_per_tx: per_tx,
                max_buy_per_day: per_day,
                max_price_impact_bps: impact_bps,
            }
            .data(),
        )
    }

    fn set_activation(&mut self, signer: &Keypair, activate_bps: u16, deactivate_bps: u16) -> bool {
        self.admin_call(signer, endowment::instruction::SetActivation { activate_bps, deactivate_bps }.data())
    }

    fn set_buy_params(&mut self, signer: &Keypair, buy_bps: u16, interval: i64, tip_bps: u16) -> bool {
        self.admin_call(
            signer,
            endowment::instruction::SetBuyParams { buy_bps, min_buy_interval_secs: interval, tip_bps }.data(),
        )
    }

    fn close_contributions(&mut self) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::CloseContributions {}.data(),
            endowment::accounts::CloseContributions {
                config: self.config(),
                authority: self.authority(),
                coin_vault: self.coin_vault(),
                coin_token_program: self.inst.coin_program,
            }
            .to_account_metas(None),
        );
        let caller = self.funded();
        send(&mut self.svm, &[ix], &caller, &[&caller])
    }

    fn begin_count(&mut self) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::BeginCount {}.data(),
            endowment::accounts::BeginCount { config: self.config() }.to_account_metas(None),
        );
        let caller = self.funded();
        send(&mut self.svm, &[ix], &caller, &[&caller])
    }

    fn count_pairs(&mut self, config: Pubkey, pairs: &[(Pubkey, Pubkey)]) -> bool {
        let mut metas = endowment::accounts::CountLandlords { config }.to_account_metas(None);
        for (landlord, coin_account) in pairs {
            metas.push(AccountMeta::new(*landlord, false));
            metas.push(AccountMeta::new_readonly(*coin_account, false));
        }
        let ix = Instruction::new_with_bytes(endowment::id(), &endowment::instruction::CountLandlords {}.data(), metas);
        let caller = self.funded();
        send(&mut self.svm, &[ix], &caller, &[&caller])
    }

    fn count(&mut self, owners: &[Pubkey]) -> bool {
        let config = self.config();
        let pairs: Vec<_> =
            owners.iter().map(|o| (landlord_pda(&config, o), self.inst.coin_account(o))).collect();
        self.count_pairs(config, &pairs)
    }

    fn finish_count(&mut self) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::FinishCount {}.data(),
            endowment::accounts::FinishCount { config: self.config(), coin_mint: self.inst.coin_mint }
                .to_account_metas(None),
        );
        let caller = self.funded();
        send(&mut self.svm, &[ix], &caller, &[&caller])
    }

    fn mint_to(&mut self, mint: &Pubkey, program: &Pubkey, account: &Pubkey, amount: u64) {
        if amount == 0 {
            return;
        }
        let ix =
            spl_token_2022::instruction::mint_to(program, mint, account, &self.mint_authority.pubkey(), &[], amount)
                .unwrap();
        let auth = self.mint_authority.insecure_clone();
        assert!(send(&mut self.svm, &[ix], &auth, &[&auth]));
    }

    fn mint_coin(&mut self, owner: &Pubkey, amount: u64) {
        let (mint, program) = (self.inst.coin_mint, self.inst.coin_program);
        let account = self.inst.coin_account(owner);
        self.mint_to(&mint, &program, &account, amount);
    }

    fn burn_coin(&mut self, owner: &Keypair, amount: u64) {
        let account = self.inst.coin_account(&owner.pubkey());
        let ix = spl_token_2022::instruction::burn(
            &self.inst.coin_program,
            &account,
            &self.inst.coin_mint,
            &owner.pubkey(),
            &[],
            amount,
        )
        .unwrap();
        assert!(send(&mut self.svm, &[ix], owner, &[owner]));
    }

    /// Simulates a dividend drop landing in a holder's account.
    fn airdrop_dividend(&mut self, account: &Pubkey, amount: u64) {
        let (mint, program) = (self.inst.dividend_mint, self.inst.dividend_program);
        self.mint_to(&mint, &program, account, amount);
    }

    /// A holder wallet with dividend and coin token accounts, holding `starting` dividend.
    fn new_landlord(&mut self, starting: u64) -> (Keypair, Pubkey) {
        let owner = Keypair::new();
        self.svm.airdrop(&owner.pubkey(), 1_000_000_000).unwrap();
        let account = self.inst.dividend_account(&owner.pubkey());
        let create = create_associated_token_account_idempotent(
            &owner.pubkey(),
            &owner.pubkey(),
            &self.inst.dividend_mint,
            &self.inst.dividend_program,
        );
        let create_coin = create_associated_token_account_idempotent(
            &owner.pubkey(),
            &owner.pubkey(),
            &self.inst.coin_mint,
            &self.inst.coin_program,
        );
        assert!(send(&mut self.svm, &[create, create_coin], &owner, &[&owner]));
        self.airdrop_dividend(&account, starting);
        (owner, account)
    }

    fn approve_ix_to(&self, owner: &Pubkey, account: &Pubkey, delegate: &Pubkey) -> Instruction {
        spl_token_2022::instruction::approve(&self.inst.dividend_program, account, delegate, owner, &[], u64::MAX)
            .unwrap()
    }

    fn approve_ix(&self, owner: &Pubkey, account: &Pubkey) -> Instruction {
        self.approve_ix_to(owner, account, &self.authority())
    }

    fn register_accounts(&self, owner: &Pubkey, account: &Pubkey) -> endowment::accounts::RegisterLandlord {
        let config = self.config();
        endowment::accounts::RegisterLandlord {
            owner: *owner,
            config,
            authority: authority_pda(&config),
            landlord: landlord_pda(&config, owner),
            dividend_mint: self.inst.dividend_mint,
            dividend_account: *account,
            coin_mint: self.inst.coin_mint,
            coin_account: self.inst.coin_account(owner),
            dividend_token_program: self.inst.dividend_program,
            coin_token_program: self.inst.coin_program,
            system_program: system_program::ID,
        }
    }

    fn register_ix_with(&self, accounts: endowment::accounts::RegisterLandlord) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::RegisterLandlord {}.data(),
            accounts.to_account_metas(None),
        )
    }

    fn register_ix(&self, owner: &Pubkey, account: &Pubkey) -> Instruction {
        self.register_ix_with(self.register_accounts(owner, account))
    }

    /// A landlord that has delegated and registered.
    fn registered_landlord(&mut self, starting: u64) -> (Keypair, Pubkey) {
        let (owner, account) = self.new_landlord(starting);
        let ixs = [self.approve_ix(&owner.pubkey(), &account), self.register_ix(&owner.pubkey(), &account)];
        assert!(send(&mut self.svm, &ixs, &owner, &[&owner]));
        (owner, account)
    }

    fn deregister_with(&mut self, owner: &Keypair, config: Pubkey, landlord: Pubkey) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::DeregisterLandlord {}.data(),
            endowment::accounts::DeregisterLandlord { owner: owner.pubkey(), config, landlord }
                .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], owner, &[owner])
    }

    fn deregister(&mut self, owner: &Keypair) -> bool {
        let config = self.config();
        self.deregister_with(owner, config, landlord_pda(&config, &owner.pubkey()))
    }

    fn sweep_accounts(&self, owner: &Pubkey, account: &Pubkey) -> endowment::accounts::Sweep {
        let config = self.config();
        endowment::accounts::Sweep {
            config,
            authority: authority_pda(&config),
            landlord: landlord_pda(&config, owner),
            dividend_mint: self.inst.dividend_mint,
            dividend_account: *account,
            dividend_vault: self.dividend_vault(),
            coin_vault: self.coin_vault(),
            dividend_token_program: self.inst.dividend_program,
            coin_token_program: self.inst.coin_program,
        }
    }

    fn sweep_with(&mut self, accounts: endowment::accounts::Sweep) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Sweep {}.data(),
            accounts.to_account_metas(None),
        );
        // Anyone can crank a sweep.
        let cranker = self.funded();
        send(&mut self.svm, &[ix], &cranker, &[&cranker])
    }

    fn sweep(&mut self, owner: &Pubkey, account: &Pubkey) -> bool {
        let accounts = self.sweep_accounts(owner, account);
        self.sweep_with(accounts)
    }

    fn pause(&mut self, signer: &Keypair) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Pause {}.data(),
            endowment::accounts::Pause { guardian: signer.pubkey(), config: self.config() }.to_account_metas(None),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn unpause(&mut self, signer: &Keypair) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Unpause {}.data(),
            endowment::accounts::Unpause { admin: signer.pubkey(), config: self.config() }.to_account_metas(None),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn set_guardian(&mut self, signer: &Keypair, new_guardian: Pubkey) -> bool {
        self.admin_call(signer, endowment::instruction::SetGuardian { new_guardian }.data())
    }

    fn propose_admin(&mut self, signer: &Keypair, new_admin: Pubkey) -> bool {
        self.admin_call(signer, endowment::instruction::ProposeAdmin { new_admin }.data())
    }

    fn accept_admin(&mut self, signer: &Keypair) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::AcceptAdmin {}.data(),
            endowment::accounts::AcceptAdmin { new_admin: signer.pubkey(), config: self.config() }
                .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], signer, &[signer])
    }

    fn config_state(&self) -> Config {
        Self::config_at(&self.svm, &self.config())
    }

    fn config_at(svm: &LiteSVM, config: &Pubkey) -> Config {
        let account = svm.get_account(config).unwrap();
        Config::try_deserialize(&mut account.data.as_slice()).unwrap()
    }

    fn landlord_state(&self, owner: &Pubkey) -> Landlord {
        let account = self.svm.get_account(&landlord_pda(&self.config(), owner)).unwrap();
        Landlord::try_deserialize(&mut account.data.as_slice()).unwrap()
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

    /// A second instance: same dividend asset, a new coin and creator, its own stand-in pool.
    fn second_instance(&mut self) -> Inst {
        let creator = self.funded();
        let coin_program = self.inst.coin_program;
        let coin_mint = create_mint(&mut self.svm, &self.mint_authority.pubkey(), &coin_program);
        let pool = fake_pool(&mut self.svm, [self.inst.dividend_mint, coin_mint], fixtures::cpmm_program());
        Inst {
            creator,
            coin_mint,
            dividend_mint: self.inst.dividend_mint,
            coin_program,
            dividend_program: self.inst.dividend_program,
            pool,
        }
    }
}

// Creation.

#[test]
fn anyone_can_create_an_endowment_and_the_creator_is_admin_by_default() {
    let mut env = Env::new();
    env.create();
    let config = env.config_state();
    assert_eq!(config.creator, env.inst.creator.pubkey());
    assert_eq!(config.admin, env.inst.creator.pubkey());
    assert_eq!(config.guardian, env.guardian.pubkey());
    assert_eq!((config.coin_mint, config.dividend_mint, config.pool), (env.inst.coin_mint, env.inst.dividend_mint, env.inst.pool));
    assert_eq!((config.activate_bps, config.deactivate_bps, config.active), (3_000, 2_500, false));
    assert_eq!((config.contribution_cap, config.closed, config.buy_bps), (CONTRIBUTION_CAP, false, 10_000));
    assert_eq!((config.min_buy_interval_secs, config.tip_bps, config.donation_bps), (600, 25, 0));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);
    assert_eq!(token_balance(&env.svm, &env.coin_vault()), 0);
}

#[test]
fn creation_is_per_creator_and_cant_be_squatted_or_repeated() {
    let mut env = Env::new();
    // Someone pre-creates the (predictable) vault accounts; creation still works.
    let griefer = env.funded();
    let inst = env.inst.clone();
    let pre = [
        create_associated_token_account_idempotent(&griefer.pubkey(), &inst.authority(), &inst.coin_mint, &inst.coin_program),
        create_associated_token_account_idempotent(
            &griefer.pubkey(),
            &inst.authority(),
            &inst.dividend_mint,
            &inst.dividend_program,
        ),
    ];
    assert!(send(&mut env.svm, &pre, &griefer, &[&griefer]));
    env.create();

    // The same creator can't create a second endowment for the same coin.
    let guardian = env.guardian.pubkey();
    assert!(!env.create_with(params(guardian, 0)));

    // Another creator for the same coin gets a separate instance, with its own vaults.
    let mut other = env.inst.clone();
    other.creator = env.funded();
    assert!(env.create_inst(&other, params(guardian, 0)));
    assert_ne!(other.config(), env.config());
    assert_ne!(other.coin_vault(), env.coin_vault());
    assert_eq!(Env::config_at(&env.svm, &other.config()).admin, other.creator.pubkey());
    assert_eq!(env.config_state().admin, env.inst.creator.pubkey());
}

#[test]
fn creation_rejects_a_pool_that_doesnt_trade_exactly_the_coin_and_dividend() {
    let mut env = Env::new();
    let guardian = env.guardian.pubkey();
    let stranger_mint = create_mint(&mut env.svm, &env.mint_authority.pubkey(), &TOKEN_2022);

    // A pool for a different pair.
    let mut wrong = env.inst.clone();
    wrong.pool = fake_pool(&mut env.svm, [env.inst.coin_mint, stranger_mint], fixtures::cpmm_program());
    assert!(!env.create_inst(&wrong, params(guardian, 0)));

    // A look-alike pool not owned by Raydium CPMM.
    let mut not_raydium = env.inst.clone();
    not_raydium.pool = fake_pool(&mut env.svm, [env.inst.coin_mint, env.inst.dividend_mint], Pubkey::new_unique());
    assert!(!env.create_inst(&not_raydium, params(guardian, 0)));

    // The coin can't be its own dividend.
    let mut same = env.inst.clone();
    same.dividend_mint = same.coin_mint;
    same.pool = fake_pool(&mut env.svm, [same.coin_mint, same.coin_mint], fixtures::cpmm_program());
    assert!(!env.create_inst(&same, params(guardian, 0)));

    // Either orientation of the real pair works.
    let mut flipped = env.inst.clone();
    flipped.pool = fake_pool(&mut env.svm, [env.inst.dividend_mint, env.inst.coin_mint], fixtures::cpmm_program());
    assert!(env.create_inst(&flipped, params(guardian, 0)));
}

#[test]
fn creation_rejects_out_of_bounds_parameters_and_donations() {
    let mut env = Env::new();
    let guardian = env.guardian.pubkey();
    let with = |f: &dyn Fn(&mut CreateParams)| {
        let mut p = params(guardian, 0);
        f(&mut p);
        p
    };
    assert!(!env.create_with(with(&|p| p.contribution_cap = 0)));
    assert!(!env.create_with(with(&|p| p.max_price_impact_bps = 301)));
    assert!(!env.create_with(with(&|p| p.max_buy_per_tx = MAX_BUY_PER_DAY + 1)));
    assert!(!env.create_with(with(&|p| p.activate_bps = 5_001)));
    assert!(!env.create_with(with(&|p| p.tip_bps = 51)));
    assert!(!env.create_with(with(&|p| p.min_buy_interval_secs = 59)));
    // Donations are only possible in the flagship's dividend asset (PUMP); this
    // coin pays a different one.
    assert!(!env.create_with(with(&|p| p.donation_bps = 10)));
    assert!(!env.create_with(with(&|p| p.donation_bps = 15)));
    assert!(env.create_with(with(&|p| p.donation_bps = 0)));
}

#[test]
fn only_fixed_donation_rates_are_accepted_for_pump_paying_coins() {
    let mut env = Env::with_pool();
    let guardian = env.guardian.pubkey();
    let mut p = params(guardian, 15);
    assert!(!env.create_with(p.clone()));
    p.donation_bps = 40;
    assert!(!env.create_with(p.clone()));
    // A 50 bps tip plus a 30 bps donation is the most allowed.
    p.donation_bps = 30;
    p.tip_bps = 50;
    assert!(env.create_with(p));
    let config = env.config_state();
    assert_eq!((config.donation_bps, config.tip_bps), (30, 50));
}

// Landlords and sweeps.

#[test]
fn register_requires_delegation() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.new_landlord(100);

    let register = env.register_ix(&owner.pubkey(), &account);
    assert!(!send(&mut env.svm, &[register.clone()], &owner, &[&owner]));

    let approve = env.approve_ix(&owner.pubkey(), &account);
    assert!(send(&mut env.svm, &[approve, register], &owner, &[&owner]));
    let landlord = env.landlord_state(&owner.pubkey());
    assert_eq!((landlord.baseline, landlord.config), (100, env.config()));
}

#[test]
fn sweeps_only_new_dividends_and_tracks_contributions() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(100);
    let program = env.inst.dividend_program;
    let mint = env.inst.dividend_mint;

    // A dividend drop lands; the sweep takes only the new amount.
    env.airdrop_dividend(&account, 250);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 250);
    assert_eq!(env.landlord_state(&owner.pubkey()).total_contributed, 250);

    // Nothing new: a sweep is a harmless no-op.
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 250);

    // The landlord spends 60 of their own; the baseline follows them down.
    let burn = spl_token_2022::instruction::burn(&program, &account, &mint, &owner.pubkey(), &[], 60).unwrap();
    assert!(send(&mut env.svm, &[burn], &owner, &[&owner]));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 40);

    env.airdrop_dividend(&account, 10);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 40);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 260);
    let landlord = env.landlord_state(&owner.pubkey());
    assert_eq!((landlord.baseline, landlord.total_contributed), (40, 260));
}

#[test]
fn drop_landing_after_a_spend_is_left_with_the_landlord() {
    // The program can't see a balance's low point between sweeps, so a drop that
    // lands after the landlord spends below their baseline, but before the next
    // sweep, is treated as the landlord's own. It errs toward the landlord.
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(100);
    let (program, mint) = (env.inst.dividend_program, env.inst.dividend_mint);

    let burn = spl_token_2022::instruction::burn(&program, &account, &mint, &owner.pubkey(), &[], 60).unwrap();
    assert!(send(&mut env.svm, &[burn], &owner, &[&owner]));
    env.airdrop_dividend(&account, 10);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 50);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 50);
}

#[test]
fn revoking_delegation_stops_sweeps() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);

    // Opting out uses the token program directly, not the endowment.
    let revoke =
        spl_token_2022::instruction::revoke(&env.inst.dividend_program, &account, &owner.pubkey(), &[]).unwrap();
    assert!(send(&mut env.svm, &[revoke], &owner, &[&owner]));

    env.airdrop_dividend(&account, 500);
    assert!(!env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 500);
}

#[test]
fn guardian_pause_blocks_sweeps_and_expires() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, 50);

    let stranger = env.funded();
    assert!(!env.pause(&stranger));

    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    assert!(!env.sweep(&owner.pubkey(), &account));

    env.warp(MAX_PAUSE_SECONDS + 1);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 50);
}

#[test]
fn guardian_pauses_but_only_admin_unpauses_early() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);

    let guardian = env.guardian.insecure_clone();
    let admin = env.admin();
    assert!(env.pause(&guardian));
    assert!(!env.unpause(&guardian));
    env.airdrop_dividend(&account, 5);
    assert!(!env.sweep(&owner.pubkey(), &account));

    assert!(env.unpause(&admin));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 5);
}

#[test]
fn admin_rotates_the_guardian() {
    let mut env = Env::new();
    env.create();
    let old_guardian = env.guardian.insecure_clone();
    let admin = env.admin();
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
    env.create();
    let old_admin = env.admin();
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

const INTERVAL: i64 = 600;

fn funded_pool_env(dividend_in_vault: u64) -> Env {
    let mut env = Env::with_pool();
    env.create();
    let vault = env.dividend_vault();
    env.set_balance(&vault, dividend_in_vault);
    let authority = env.authority();
    env.create_lp_vault(&authority);
    env
}

fn tip_on(amount: u64) -> u64 {
    amount * 25 / 10_000
}

#[test]
fn buyback_sizes_itself_and_pays_the_caller_a_tip() {
    let mut env = funded_pool_env(50_000 * UNIT);
    let pool_dividend_before = token_balance(&env.svm, &fixtures::key(fixtures::POOL_PUMP_VAULT));

    let (ok, caller) = env.buyback(1);
    assert!(ok);

    // The contract spent the per-transaction cap, not a caller-chosen amount.
    let received = token_balance(&env.svm, &env.coin_vault());
    assert!(received > 0);
    let tip = tip_on(MAX_BUY_PER_TX);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 50_000 * UNIT - MAX_BUY_PER_TX - tip);
    assert_eq!(token_balance(&env.svm, &env.inst.dividend_account(&caller.pubkey())), tip);
    assert_eq!(
        token_balance(&env.svm, &fixtures::key(fixtures::POOL_PUMP_VAULT)),
        pool_dividend_before + MAX_BUY_PER_TX
    );
    let config = env.config_state();
    assert_eq!(config.total_dividend_spent, MAX_BUY_PER_TX);
    assert_eq!(config.total_coin_bought, received);
    assert_eq!(config.total_tips, tip);
    assert_eq!(config.total_donated, 0);
    assert_eq!(config.bought_today, MAX_BUY_PER_TX);
}

#[test]
fn buyback_spends_what_the_vault_holds_when_below_the_cap() {
    let mut env = funded_pool_env(1_002_500_000); // 1,000 + a 25 bps tip
    assert!(env.buy());
    assert_eq!(env.config_state().total_dividend_spent, 1_000 * UNIT);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);

    // An empty vault has nothing to buy.
    env.warp(INTERVAL);
    assert!(!env.buy());
}

#[test]
fn buyback_enforces_spacing_and_the_daily_cap() {
    let mut env = funded_pool_env(100_000 * UNIT);
    assert!(env.buy());
    // Too soon after the last buy.
    assert!(!env.buy());
    env.warp(INTERVAL - 1);
    assert!(!env.buy());

    for _ in 0..3 {
        env.warp(INTERVAL);
        assert!(env.buy());
    }
    // Four buys hit the 20,000 daily cap.
    env.warp(INTERVAL);
    assert!(!env.buy());

    env.warp(24 * 60 * 60);
    assert!(env.buy());
    assert_eq!(env.config_state().bought_today, MAX_BUY_PER_TX);
}

#[test]
fn buyback_is_blocked_while_paused() {
    let mut env = funded_pool_env(10_000 * UNIT);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    assert!(!env.buy());
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 10_000 * UNIT);
}

#[test]
fn buyback_does_not_need_landlord_activation() {
    let mut env = funded_pool_env(10_000 * UNIT);
    assert!(!env.config_state().active);
    assert!(env.buy());
}

#[test]
fn buyback_rejects_bad_fills() {
    let mut env = funded_pool_env(5_000_000 * UNIT);

    // The caller's own minimum is enforced.
    assert!(!env.buyback(u64::MAX).0);

    // A trade big enough to move the price well past 1% is refused, even with caps raised.
    let admin = env.admin();
    assert!(env.set_buyback_limits(&admin, 3_000_000 * UNIT, 3_000_000 * UNIT, MAX_PRICE_IMPACT_BPS));
    assert!(!env.buy());
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 5_000_000 * UNIT);
    assert_eq!(token_balance(&env.svm, &env.coin_vault()), 0);
}

#[test]
fn buyback_rejects_accounts_from_other_pools_and_foreign_tip_accounts() {
    let mut env = funded_pool_env(10_000 * UNIT);
    let caller = env.cranker();

    let mut wrong_pool = env.buyback_accounts(&caller.pubkey());
    wrong_pool.pool_state = fixtures::key(fixtures::AMM_CONFIG);
    assert!(!env.buyback_with(&caller, wrong_pool, 1));

    // The pool's two vaults swapped: the dividend would flow the wrong way.
    let mut swapped = env.buyback_accounts(&caller.pubkey());
    std::mem::swap(&mut swapped.pool_dividend_vault, &mut swapped.pool_coin_vault);
    assert!(!env.buyback_with(&caller, swapped, 1));

    // Output can only land in the endowment's own coin vault, not the caller's.
    let create = create_associated_token_account_idempotent(
        &caller.pubkey(),
        &caller.pubkey(),
        &env.inst.coin_mint,
        &env.inst.coin_program,
    );
    assert!(send(&mut env.svm, &[create], &caller, &[&caller]));
    let mut elsewhere = env.buyback_accounts(&caller.pubkey());
    elsewhere.coin_vault = env.inst.coin_account(&caller.pubkey());
    assert!(!env.buyback_with(&caller, elsewhere, 1));

    // The tip can only go to the caller's own dividend account.
    let other = env.cranker();
    let mut foreign_tip = env.buyback_accounts(&caller.pubkey());
    foreign_tip.caller_dividend_account = env.inst.dividend_account(&other.pubkey());
    assert!(!env.buyback_with(&caller, foreign_tip, 1));

    let accounts = env.buyback_accounts(&caller.pubkey());
    assert!(env.buyback_with(&caller, accounts, 1));
}

#[test]
fn buyback_limits_are_admin_only_and_bounded() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let stranger = env.funded();
    assert!(!env.set_buyback_limits(&stranger, UNIT, UNIT, 100));
    assert!(!env.set_buyback_limits(&admin, UNIT, UNIT, 301));
    assert!(!env.set_buyback_limits(&admin, 2 * UNIT, UNIT, 100));
    assert!(env.set_buyback_limits(&admin, UNIT, 10 * UNIT, 50));
    let config = env.config_state();
    assert_eq!((config.max_buy_per_tx, config.max_buy_per_day, config.max_price_impact_bps), (UNIT, 10 * UNIT, 50));
}

#[test]
fn buy_params_are_admin_only_and_bounded() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let stranger = env.funded();
    assert!(!env.set_buy_params(&stranger, 5_000, 600, 25));
    assert!(!env.set_buy_params(&admin, 10_001, 600, 25));
    assert!(!env.set_buy_params(&admin, 5_000, 59, 25));
    assert!(!env.set_buy_params(&admin, 5_000, 24 * 60 * 60 + 1, 25));
    assert!(!env.set_buy_params(&admin, 5_000, 600, 51));
    assert!(env.set_buy_params(&admin, 6_000, 900, 50));
    let config = env.config_state();
    assert_eq!((config.buy_bps, config.min_buy_interval_secs, config.tip_bps), (6_000, 900, 50));
}

// Donations to the flagship endowment.

#[test]
fn a_donating_endowment_sends_its_share_to_the_flagship_vault() {
    let mut env = Env::with_pool();
    let guardian = env.guardian.pubkey();
    assert!(env.create_with(params(guardian, 20)));
    let vault = env.dividend_vault();
    env.set_balance(&vault, 50_000 * UNIT);
    let authority = env.authority();
    env.create_lp_vault(&authority);

    // The flagship's dividend vault (normally created with the flagship endowment).
    let payer = env.funded();
    let flagship_authority = authority_pda(&FLAGSHIP_CONFIG);
    let create = create_associated_token_account_idempotent(
        &payer.pubkey(),
        &flagship_authority,
        &env.inst.dividend_mint,
        &env.inst.dividend_program,
    );
    assert!(send(&mut env.svm, &[create], &payer, &[&payer]));

    // The donation can only go to the flagship's vault.
    let caller = env.cranker();
    let mut elsewhere = env.buyback_accounts(&caller.pubkey());
    elsewhere.flagship_dividend_vault = env.inst.dividend_account(&caller.pubkey());
    assert!(!env.buyback_with(&caller, elsewhere, 1));

    let accounts = env.buyback_accounts(&caller.pubkey());
    assert!(env.buyback_with(&caller, accounts, 1));

    let donation = MAX_BUY_PER_TX * 20 / 10_000;
    let tip = tip_on(MAX_BUY_PER_TX);
    assert_eq!(token_balance(&env.svm, &env.flagship_vault()), donation);
    assert_eq!(token_balance(&env.svm, &env.inst.dividend_account(&caller.pubkey())), tip);
    assert_eq!(
        token_balance(&env.svm, &env.dividend_vault()),
        50_000 * UNIT - MAX_BUY_PER_TX - tip - donation
    );
    let config = env.config_state();
    assert_eq!((config.total_donated, config.total_tips, config.total_dividend_spent), (donation, tip, MAX_BUY_PER_TX));
}

#[test]
fn a_donating_endowment_leaves_room_for_the_donation_when_sizing() {
    let mut env = Env::with_pool();
    let guardian = env.guardian.pubkey();
    assert!(env.create_with(params(guardian, 30)));
    // 1,000 + 25 bps tip + 30 bps donation.
    let vault = env.dividend_vault();
    env.set_balance(&vault, 1_005_500_000);
    let authority = env.authority();
    env.create_lp_vault(&authority);
    let payer = env.funded();
    let create = create_associated_token_account_idempotent(
        &payer.pubkey(),
        &authority_pda(&FLAGSHIP_CONFIG),
        &env.inst.dividend_mint,
        &env.inst.dividend_program,
    );
    assert!(send(&mut env.svm, &[create], &payer, &[&payer]));

    assert!(env.buy());
    assert_eq!(env.config_state().total_dividend_spent, 1_000 * UNIT);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);
    assert_eq!(token_balance(&env.svm, &env.flagship_vault()), 3 * UNIT);
}

// Isolation between endowments.

#[test]
fn landlords_of_one_endowment_cant_be_registered_swept_counted_or_removed_through_another() {
    let mut env = Env::new();
    env.create_active();
    let a = env.inst.clone();
    let b = env.second_instance();
    let guardian = env.guardian.pubkey();
    let mut p = params(guardian, 0);
    p.activate_bps = 0;
    p.deactivate_bps = 0;
    assert!(env.create_inst(&b, p));

    // A landlord of A.
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, 100);

    // Registering with B's config but A's delegation fails: B's authority isn't the delegate.
    let (other, other_account) = env.new_landlord(0);
    let approve_a = env.approve_ix(&other.pubkey(), &other_account);
    env.inst = b.clone();
    let create_b_coin = create_associated_token_account_idempotent(
        &other.pubkey(),
        &other.pubkey(),
        &b.coin_mint,
        &b.coin_program,
    );
    assert!(send(&mut env.svm, &[create_b_coin], &other, &[&other]));
    let register_b = env.register_ix(&other.pubkey(), &other_account);
    assert!(!send(&mut env.svm, &[approve_a, register_b], &other, &[&other]));
    // Nor can A's authority be passed in B's place.
    let mut mixed = env.register_accounts(&other.pubkey(), &other_account);
    mixed.authority = a.authority();
    let ix = env.register_ix_with(mixed);
    let approve_a = spl_token_2022::instruction::approve(
        &a.dividend_program,
        &other_account,
        &a.authority(),
        &other.pubkey(),
        &[],
        u64::MAX,
    )
    .unwrap();
    assert!(!send(&mut env.svm, &[approve_a, ix], &other, &[&other]));
    // Delegating to B's own authority is what registering with B takes.
    let approve_b = env.approve_ix(&other.pubkey(), &other_account);
    let register_b = env.register_ix(&other.pubkey(), &other_account);
    assert!(send(&mut env.svm, &[approve_b, register_b], &other, &[&other]));
    assert_eq!(env.landlord_state(&other.pubkey()).config, b.config());
    let b_landlords = 1;

    // Sweeping A's landlord through B's config, authority or vault fails.
    env.inst = a.clone();
    let mut via_b = env.sweep_accounts(&owner.pubkey(), &account);
    via_b.config = b.config();
    via_b.authority = b.authority();
    via_b.dividend_vault = b.dividend_vault();
    via_b.coin_vault = b.coin_vault();
    assert!(!env.sweep_with(via_b));
    let mut into_b_vault = env.sweep_accounts(&owner.pubkey(), &account);
    into_b_vault.dividend_vault = b.dividend_vault();
    assert!(!env.sweep_with(into_b_vault));
    let mut b_authority = env.sweep_accounts(&owner.pubkey(), &account);
    b_authority.authority = b.authority();
    assert!(!env.sweep_with(b_authority));
    assert_eq!(token_balance(&env.svm, &b.dividend_vault()), 0);

    // The legitimate sweep into A works.
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &a.dividend_vault()), 100);

    // B's count can't include A's landlord.
    env.inst = b.clone();
    assert!(env.begin_count());
    let landlord_a = landlord_pda(&a.config(), &owner.pubkey());
    let coin_a = a.coin_account(&owner.pubkey());
    assert!(!env.count_pairs(b.config(), &[(landlord_a, coin_a)]));

    // A's landlord can't be deregistered through B.
    assert!(!env.deregister_with(&owner, b.config(), landlord_a));
    env.inst = a.clone();
    assert_eq!(env.config_state().landlord_count, 1);
    assert!(env.deregister(&owner));
    assert_eq!(env.config_state().landlord_count, 0);
    assert_eq!(Env::config_at(&env.svm, &b.config()).landlord_count, b_landlords);
}

#[test]
fn two_endowments_on_one_pool_keep_separate_vaults() {
    let mut env = Env::with_pool();
    env.create();
    let a = env.inst.clone();
    let mut b = env.inst.clone();
    b.creator = env.funded();
    let guardian = env.guardian.pubkey();
    assert!(env.create_inst(&b, params(guardian, 0)));
    for inst in [&a, &b] {
        let vault = inst.dividend_vault();
        env.set_balance(&vault, 10_000 * UNIT);
        env.create_lp_vault(&inst.authority());
    }

    let caller = env.cranker();
    // A's buyback can't send its coin to B's vault or spend B's dividend.
    let mut to_b = env.buyback_accounts_for(&a, &caller.pubkey());
    to_b.coin_vault = b.coin_vault();
    assert!(!env.buyback_with(&caller, to_b, 1));
    let mut from_b = env.buyback_accounts_for(&a, &caller.pubkey());
    from_b.dividend_vault = b.dividend_vault();
    assert!(!env.buyback_with(&caller, from_b, 1));
    // Nor sign with B's authority against A's config.
    let mut b_authority = env.buyback_accounts_for(&a, &caller.pubkey());
    b_authority.authority = b.authority();
    assert!(!env.buyback_with(&caller, b_authority, 1));
    assert_eq!(token_balance(&env.svm, &b.dividend_vault()), 10_000 * UNIT);

    // Each buys for itself.
    let accounts = env.buyback_accounts_for(&a, &caller.pubkey());
    assert!(env.buyback_with(&caller, accounts, 1));
    let accounts = env.buyback_accounts_for(&b, &caller.pubkey());
    assert!(env.buyback_with(&caller, accounts, 1));
    assert!(token_balance(&env.svm, &a.coin_vault()) > 0);
    assert!(token_balance(&env.svm, &b.coin_vault()) > 0);
    assert_eq!(Env::config_at(&env.svm, &a.config()).total_dividend_spent, MAX_BUY_PER_TX);
    assert_eq!(Env::config_at(&env.svm, &b.config()).total_dividend_spent, MAX_BUY_PER_TX);
}

// Original SPL Token mints (not Token-2022).

#[test]
fn endowments_work_with_original_spl_token_mints() {
    let mut env = Env::with_programs(TOKEN, TOKEN);
    env.create_active();
    let (owner, account) = env.registered_landlord(10);
    env.airdrop_dividend(&account, 90);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 90);
    assert_eq!(token_balance(&env.svm, &account), 10);

    env.mint_coin(&owner.pubkey(), 400);
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 600);
    assert!(env.begin_count());
    assert!(env.count(&[owner.pubkey()]));
    assert!(env.finish_count());
    assert_eq!(env.config_state().count.last_committed_bps, 4_000);
}

#[test]
fn a_token_2022_coin_can_pay_an_original_spl_token_dividend() {
    let mut env = Env::with_programs(TOKEN_2022, TOKEN);
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, 25);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 25);
}

// Activation: the daily commitment count.

/// Three landlords holding 10%, 15% and 5% of a 1,000,000 coin supply.
fn counted_env() -> (Env, Vec<Keypair>) {
    let mut env = Env::new();
    env.create();
    let mut owners = vec![];
    for share in [100_000, 150_000, 50_000] {
        let (owner, _) = env.registered_landlord(0);
        env.mint_coin(&owner.pubkey(), share * UNIT);
        owners.push(owner);
    }
    // The rest of the supply sits with someone who isn't a landlord.
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 700_000 * UNIT);
    (env, owners)
}

fn keys(owners: &[Keypair]) -> Vec<Pubkey> {
    owners.iter().map(|o| o.pubkey()).collect()
}

#[test]
fn sweeps_wait_for_activation() {
    let mut env = Env::new();
    env.create();
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, 50);
    assert!(!env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 50);
}

#[test]
fn count_switches_sweeps_on_at_30_percent_with_hysteresis() {
    let (mut env, owners) = counted_env();

    // 30% committed: on.
    assert!(env.begin_count());
    assert!(env.count(&keys(&owners)));
    assert!(env.finish_count());
    let config = env.config_state();
    assert_eq!((config.count.last_committed_bps, config.active), (3_000, true));

    // Down to 27%: between the lines, so it stays on.
    env.burn_coin(&owners[0], 30_000 * UNIT);
    env.warp(24 * 60 * 60);
    assert!(env.begin_count());
    assert!(env.count(&keys(&owners)));
    assert!(env.finish_count());
    let config = env.config_state();
    assert_eq!((config.count.last_committed_bps, config.active), (2_783, true));

    // Down to about 22%: below 25%, off.
    env.burn_coin(&owners[1], 50_000 * UNIT);
    env.warp(24 * 60 * 60);
    assert!(env.begin_count());
    assert!(env.count(&keys(&owners)));
    assert!(env.finish_count());
    assert!(!env.config_state().active);

    // And sweeps stop.
    let owner = owners[2].insecure_clone();
    let account = env.inst.dividend_account(&owner.pubkey());
    env.airdrop_dividend(&account, 10);
    assert!(!env.sweep(&owner.pubkey(), &account));
}

#[test]
fn count_runs_once_a_day_and_counts_each_landlord_once() {
    let (mut env, owners) = counted_env();
    assert!(env.begin_count());
    // One round at a time.
    assert!(!env.begin_count());

    // Can't finish until every landlord is counted, and can't count one twice.
    assert!(env.count(&keys(&owners[..2])));
    assert!(!env.finish_count());
    assert!(!env.count(&keys(&owners[..1])));
    assert!(!env.count(&[owners[2].pubkey(), owners[2].pubkey()]));
    assert!(env.count(&keys(&owners[2..])));
    assert!(env.finish_count());
    assert!(!env.finish_count());

    // Once a day.
    env.warp(24 * 60 * 60 - 1);
    assert!(!env.begin_count());
    env.warp(1);
    assert!(env.begin_count());
}

#[test]
fn count_rejects_accounts_that_are_not_the_landlords_own() {
    let (mut env, owners) = counted_env();
    assert!(env.begin_count());
    let config = env.config();

    // A landlord PDA paired with someone else's coin account.
    let landlord = landlord_pda(&config, &owners[0].pubkey());
    let theirs = env.inst.coin_account(&owners[1].pubkey());
    assert!(!env.count_pairs(config, &[(landlord, theirs)]));

    // A coin account posing as a landlord.
    assert!(!env.count_pairs(config, &[(theirs, theirs)]));
}

#[test]
fn landlords_joining_or_leaving_mid_count_cant_stall_or_skew_it() {
    let (mut env, owners) = counted_env();
    assert!(env.begin_count());
    assert!(env.count(&keys(&owners[..1])));

    // A new landlord joins mid-round: it sits the round out.
    let (late, _) = env.registered_landlord(0);
    env.mint_coin(&late.pubkey(), 50_000 * UNIT);
    assert!(!env.count(&[late.pubkey()]));

    // An uncounted landlord leaves: the round no longer waits for it.
    assert!(env.deregister(&owners[2]));
    // A counted landlord leaves: its coin comes back out.
    assert!(env.deregister(&owners[0]));

    assert!(!env.finish_count());
    assert!(env.count(&keys(&owners[1..2])));
    assert!(env.finish_count());
    let config = env.config_state();
    // Only owners[1] (150,000 of 1,050,000) counts.
    assert_eq!(config.count.last_committed_bps, 1_428);
    assert_eq!(config.landlord_count, 2);

    // Next round, the late joiner is counted.
    env.warp(24 * 60 * 60);
    assert!(env.begin_count());
    assert!(env.count(&[owners[1].pubkey(), late.pubkey()]));
    assert!(env.finish_count());
    assert_eq!(env.config_state().count.last_committed_bps, 1_904);
}

#[test]
fn a_closed_coin_account_counts_as_zero() {
    let (mut env, owners) = counted_env();
    env.burn_coin(&owners[2], 50_000 * UNIT);
    let account = env.inst.coin_account(&owners[2].pubkey());
    let close = spl_token_2022::instruction::close_account(
        &env.inst.coin_program,
        &account,
        &owners[2].pubkey(),
        &owners[2].pubkey(),
        &[],
    )
    .unwrap();
    assert!(send(&mut env.svm, &[close], &owners[2], &[&owners[2]]));

    assert!(env.begin_count());
    assert!(env.count(&keys(&owners)));
    assert!(env.finish_count());
    assert_eq!(env.config_state().count.last_committed_bps, 2_631);
}

#[test]
fn activation_is_admin_only_and_bounded() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let stranger = env.funded();
    assert!(!env.set_activation(&stranger, 0, 0));
    assert!(!env.set_activation(&admin, 5_001, 2_500));
    assert!(!env.set_activation(&admin, 2_000, 2_500));
    assert!(env.set_activation(&admin, 4_000, 3_500));
    assert!(!env.config_state().active);
    // Zero turns sweeps on at once, for a founders-only test window.
    assert!(env.set_activation(&admin, 0, 0));
    assert!(env.config_state().active);
}

// Contributions close at the cap, or early by the admin, for good.

#[test]
fn contributions_close_at_the_cap() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);

    // Not yet.
    assert!(!env.close_contributions());
    let vault = env.coin_vault();
    env.set_balance(&vault, CONTRIBUTION_CAP);
    assert!(env.close_contributions());
    assert!(env.config_state().closed);

    env.airdrop_dividend(&account, 10);
    assert!(!env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 10);
}

#[test]
fn a_sweep_notices_the_cap_and_closes() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    let vault = env.coin_vault();
    env.set_balance(&vault, CONTRIBUTION_CAP + 1);
    env.airdrop_dividend(&account, 10);

    // The sweep records the close instead of moving the dividend.
    assert!(env.sweep(&owner.pubkey(), &account));
    assert!(env.config_state().closed);
    assert_eq!(token_balance(&env.svm, &account), 10);
    assert!(!env.sweep(&owner.pubkey(), &account));
}

#[test]
fn retire_closes_contributions_early_and_is_admin_only() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    let admin = env.admin();
    let stranger = env.funded();

    assert!(!env.admin_call(&stranger, endowment::instruction::Retire {}.data()));
    assert!(env.admin_call(&admin, endowment::instruction::Retire {}.data()));
    assert!(env.config_state().closed);
    env.airdrop_dividend(&account, 10);
    assert!(!env.sweep(&owner.pubkey(), &account));
}

#[test]
fn after_close_part_of_each_buy_becomes_locked_liquidity() {
    let mut env = funded_pool_env(50_000 * UNIT);
    let admin = env.admin();
    assert!(env.admin_call(&admin, endowment::instruction::Retire {}.data()));
    assert!(env.set_buy_params(&admin, 5_000, INTERVAL, 25));

    let coin_before = token_balance(&env.svm, &env.coin_vault());
    let lp_supply_before = {
        let data = env.svm.get_account(&env.inst.pool).unwrap().data;
        u64::from_le_bytes(data[333..341].try_into().unwrap())
    };
    assert!(env.buy());

    let config = env.config_state();
    let lp = token_balance(&env.svm, &env.lp_vault());
    assert!(lp > 0);
    assert_eq!(config.total_lp_tokens, lp);
    // Half the buy went to liquidity: a quarter swapped, a quarter deposited as the dividend.
    assert!(config.total_liquidity_dividend > 0 && config.total_liquidity_dividend <= MAX_BUY_PER_TX / 4);
    let data = env.svm.get_account(&env.inst.pool).unwrap().data;
    assert_eq!(u64::from_le_bytes(data[333..341].try_into().unwrap()), lp_supply_before + lp);
    // The coin vault never shrinks.
    assert!(token_balance(&env.svm, &env.coin_vault()) >= coin_before);
    assert!(token_balance(&env.svm, &env.coin_vault()) > 0);
}

#[test]
fn liquidity_can_only_land_in_the_authoritys_lp_account() {
    let mut env = funded_pool_env(50_000 * UNIT);
    let admin = env.admin();
    assert!(env.admin_call(&admin, endowment::instruction::Retire {}.data()));
    assert!(env.set_buy_params(&admin, 0, INTERVAL, 25));

    let caller = env.cranker();
    let mut accounts = env.buyback_accounts(&caller.pubkey());
    // The caller's own LP account instead of the endowment's.
    let create = create_associated_token_account_idempotent(
        &caller.pubkey(),
        &caller.pubkey(),
        &fixtures::key(fixtures::LP_MINT),
        &TOKEN,
    );
    assert!(send(&mut env.svm, &[create], &caller, &[&caller]));
    accounts.lp_vault = ata(&caller.pubkey(), &fixtures::key(fixtures::LP_MINT), &TOKEN);
    assert!(!env.buyback_with(&caller, accounts, 1));

    let accounts = env.buyback_accounts(&caller.pubkey());
    assert!(env.buyback_with(&caller, accounts, 1));
    assert!(token_balance(&env.svm, &env.lp_vault()) > 0);
}

#[test]
fn renounced_admin_freezes_every_parameter() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let stranger = env.funded();
    assert!(!env.admin_call(&stranger, endowment::instruction::RenounceAdmin {}.data()));
    assert!(env.propose_admin(&admin, stranger.pubkey()));
    assert!(env.admin_call(&admin, endowment::instruction::RenounceAdmin {}.data()));

    let config = env.config_state();
    assert_eq!((config.admin, config.pending_admin), (Pubkey::default(), Pubkey::default()));
    assert!(!env.accept_admin(&stranger));
    assert!(!env.set_buyback_limits(&admin, UNIT, UNIT, 100));
    assert!(!env.set_activation(&admin, 0, 0));
    assert!(!env.admin_call(&admin, endowment::instruction::Retire {}.data()));
}
