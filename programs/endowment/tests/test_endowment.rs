use {
    anchor_lang::{
        error::ERROR_CODE_OFFSET,
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
        constants::{
            AUTHORITY_SEED, CONFIG_SEED, COUNT_INTERVAL_SECS, FLAGSHIP_CONFIG, LANDLORD_SEED, MAX_LANDLORDS,
            MAX_PAUSE_SECONDS, PARAM_TIMELOCK_SECONDS, PAUSE_COOLDOWN_SECONDS, ROSTER_SEED,
        },
        error::EndowmentError,
        state::{Config, CreateParams, Landlord, Params, Roster},
    },
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    spl_token_2022::state::{Account as TokenAccount, Mint},
    std::cell::Cell,
};

const DECIMALS: u8 = 6;
const UNIT: u64 = 1_000_000; // one whole token in base units
const MAX_BUY_PER_TX: u64 = 5_000 * UNIT;
const MAX_BUY_PER_DAY: u64 = 20_000 * UNIT;
const MAX_PRICE_IMPACT_BPS: u16 = 100;
const CONTRIBUTION_CAP: u64 = 200_000_000 * UNIT;
const INTERVAL: i64 = 600;
const DAY: i64 = 24 * 60 * 60;

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

// Raw offsets into fixture accounts.
/// PENIS mint TransferFeeConfig: older and newer fee basis points.
const PENIS_OLDER_FEE_BPS: usize = 326;
const PENIS_NEWER_FEE_BPS: usize = 344;
/// Raydium AmmConfig trade fee rate (u64, parts per million).
const AMM_TRADE_FEE_RATE: usize = 12;
/// Raydium PoolState status bits and token-0 protocol fees.
const POOL_STATUS: usize = 329;
const POOL_PROTOCOL_FEES_0: usize = 341;
/// Raydium ObservationState: first observation and last-update timestamp.
const OBSERVATIONS: usize = 43;
const OBSERVATION_LAST_UPDATE: usize = 4_043;

// The last transaction's outcome, per test thread.
thread_local! {
    static LAST_ERR: Cell<Option<u32>> = const { Cell::new(None) };
    static LAST_CU: Cell<u64> = const { Cell::new(0) };
}

fn last_err() -> Option<u32> {
    LAST_ERR.with(|c| c.get())
}

fn last_cu() -> u64 {
    LAST_CU.with(|c| c.get())
}

fn code(err: EndowmentError) -> u32 {
    ERROR_CODE_OFFSET + err as u32
}

/// The call must fail, with exactly this program error.
macro_rules! assert_err {
    ($call:expr, $err:ident) => {{
        assert!(!$call, "expected {} but the transaction succeeded", stringify!($err));
        assert_eq!(last_err(), Some(code(EndowmentError::$err)), "expected {}", stringify!($err));
    }};
}

fn custom_code(debug: &str) -> Option<u32> {
    let at = debug.find("Custom(")? + "Custom(".len();
    debug[at..].split(')').next()?.parse().ok()
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

fn roster_pda(config: &Pubkey) -> Pubkey {
    pda(&[ROSTER_SEED, config.as_ref()])
}

fn landlord_pda(config: &Pubkey, owner: &Pubkey) -> Pubkey {
    pda(&[LANDLORD_SEED, config.as_ref(), owner.as_ref()])
}

fn send(svm: &mut LiteSVM, ixs: &[Instruction], payer: &Keypair, signers: &[&Keypair]) -> bool {
    let msg = Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &svm.latest_blockhash());
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    let result = svm.send_transaction(tx);
    match &result {
        Ok(meta) => {
            LAST_ERR.with(|c| c.set(None));
            LAST_CU.with(|c| c.set(meta.compute_units_consumed));
        }
        Err(failed) => {
            // Captured by the test harness; shown only when a test fails.
            eprintln!("tx failed: {:?}\n{}", failed.err, failed.meta.logs.join("\n"));
            LAST_ERR.with(|c| c.set(custom_code(&format!("{:?}", failed.err))));
        }
    }
    svm.expire_blockhash();
    result.is_ok()
}

fn compute_limit_ix(units: u32) -> Instruction {
    let program = fixtures::key("ComputeBudget111111111111111111111111111111");
    let mut data = vec![2u8];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction { program_id: program, accounts: vec![], data }
}

fn token_balance(svm: &LiteSVM, account: &Pubkey) -> u64 {
    TokenAccount::unpack(&svm.get_account(account).unwrap().data[..TokenAccount::LEN])
        .unwrap()
        .amount
}

fn exists(svm: &LiteSVM, account: &Pubkey) -> bool {
    svm.get_account(account).is_some_and(|a| a.lamports > 0)
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

/// The defaults the flagship launches with.
fn base_params() -> Params {
    Params {
        max_buy_per_tx: MAX_BUY_PER_TX,
        max_buy_per_day: MAX_BUY_PER_DAY,
        max_price_impact_bps: MAX_PRICE_IMPACT_BPS,
        min_buy_amount: 0,
        min_buy_interval_secs: INTERVAL,
        tip_bps: 25,
        buy_bps: 10_000,
        activate_bps: 3_000,
        deactivate_bps: 2_500,
        min_stake_bps: 10,
    }
}

fn params(guardian: Pubkey, donation_bps: u16) -> CreateParams {
    CreateParams {
        admin: Pubkey::default(),
        guardian,
        params: base_params(),
        contribution_cap: CONTRIBUTION_CAP,
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
    fn roster(&self) -> Pubkey {
        roster_pda(&self.config())
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
        // A realistic clock (LiteSVM starts at 0, which the program reads as "never").
        let mut clock: Clock = svm.get_sysvar();
        clock.unix_timestamp = 1_790_000_000;
        svm.set_sysvar(&clock);
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
    fn roster(&self) -> Pubkey {
        self.inst.roster()
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
    fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
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
                roster: roster_pda(&config),
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

    /// Create with the defaults, adjusted by `f`.
    fn create_custom(&mut self, f: impl FnOnce(&mut CreateParams)) {
        let mut p = params(self.guardian.pubkey(), 0);
        f(&mut p);
        assert!(self.create_with(p));
    }

    fn create(&mut self) {
        self.create_custom(|_| {});
    }

    /// Create with the activation threshold at 0, as for a founders-only test window.
    fn create_active(&mut self) {
        self.create_custom(|p| {
            p.params.activate_bps = 0;
            p.params.deactivate_bps = 0;
        });
        assert!(self.config_state().active);
    }

    /// Writes a token account's balance directly (the real mints can't be minted).
    fn set_balance(&mut self, account: &Pubkey, amount: u64) {
        let mut acc = self.svm.get_account(account).unwrap();
        acc.data[64..72].copy_from_slice(&amount.to_le_bytes());
        self.svm.set_account(*account, acc).unwrap();
    }

    /// Edits an account's raw data in place.
    fn poke(&mut self, account: &Pubkey, f: impl FnOnce(&mut Vec<u8>)) {
        let mut acc = self.svm.get_account(account).unwrap();
        f(&mut acc.data);
        self.svm.set_account(*account, acc).unwrap();
    }

    fn lp_vault_for(authority: &Pubkey) -> Pubkey {
        ata(authority, &fixtures::key(fixtures::LP_MINT), &TOKEN)
    }

    fn lp_vault(&self) -> Pubkey {
        Self::lp_vault_for(&self.authority())
    }

    fn lp_supply(&self) -> u64 {
        let data = self.svm.get_account(&self.inst.pool).unwrap().data;
        u64::from_le_bytes(data[333..341].try_into().unwrap())
    }

    fn flagship_vault(&self) -> Pubkey {
        ata(&authority_pda(&FLAGSHIP_CONFIG), &self.inst.dividend_mint, &self.inst.dividend_program)
    }

    fn create_flagship_vault(&mut self) {
        let payer = self.funded();
        let create = create_associated_token_account_idempotent(
            &payer.pubkey(),
            &authority_pda(&FLAGSHIP_CONFIG),
            &self.inst.dividend_mint,
            &self.inst.dividend_program,
        );
        assert!(send(&mut self.svm, &[create], &payer, &[&payer]));
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

    fn buyback_ix(&self, caller: &Pubkey, min_out: u64) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::Buyback { min_out }.data(),
            self.buyback_accounts(caller).to_account_metas(None),
        )
    }

    /// Creates the (off-curve) LP account an instance's authority holds liquidity in.
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

    fn propose(&mut self, signer: &Keypair, params: Params) -> bool {
        self.admin_call(signer, endowment::instruction::ProposeParams { params }.data())
    }

    fn cancel_params(&mut self, signer: &Keypair) -> bool {
        self.admin_call(signer, endowment::instruction::CancelParams {}.data())
    }

    /// Permissionless: anyone applies a proposal once its timelock has passed.
    fn apply_params(&mut self) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::ApplyParams {}.data(),
            endowment::accounts::ApplyParams { config: self.config() }.to_account_metas(None),
        );
        let caller = self.funded();
        send(&mut self.svm, &[ix], &caller, &[&caller])
    }

    /// Propose, wait out the timelock, apply.
    fn change_params(&mut self, f: impl FnOnce(&mut Params)) {
        let mut p = self.config_state().params;
        f(&mut p);
        let admin = self.admin();
        assert!(self.propose(&admin, p));
        self.warp(PARAM_TIMELOCK_SECONDS);
        assert!(self.apply_params());
        assert_eq!(self.config_state().params, p);
    }

    fn retire(&mut self, signer: &Keypair) -> bool {
        self.admin_call(signer, endowment::instruction::Retire {}.data())
    }

    fn renounce(&mut self, signer: &Keypair) -> bool {
        self.admin_call(signer, endowment::instruction::RenounceAdmin {}.data())
    }

    fn roster_state(&self) -> Roster {
        let account = self.svm.get_account(&self.roster()).unwrap();
        Roster::try_deserialize(&mut account.data.as_slice()).unwrap()
    }

    /// The count's remaining accounts for the current roster, in order.
    fn roster_pairs(&self) -> Vec<(Pubkey, Pubkey)> {
        self.roster_state().entries.iter().map(|e| (e.coin_account, e.dividend_account)).collect()
    }

    fn count_ix_with(&self, config: Pubkey, pairs: &[(Pubkey, Pubkey)]) -> Instruction {
        let mut metas = endowment::accounts::CountCommitment {
            config,
            roster: roster_pda(&config),
            coin_mint: Env::config_at(&self.svm, &config).coin_mint,
        }
        .to_account_metas(None);
        for (coin, dividend) in pairs {
            metas.push(AccountMeta::new_readonly(*coin, false));
            metas.push(AccountMeta::new_readonly(*dividend, false));
        }
        Instruction::new_with_bytes(endowment::id(), &endowment::instruction::CountCommitment {}.data(), metas)
    }

    fn count_ix(&self) -> Instruction {
        self.count_ix_with(self.config(), &self.roster_pairs())
    }

    fn count_with(&mut self, config: Pubkey, pairs: &[(Pubkey, Pubkey)]) -> bool {
        let ixs = [compute_limit_ix(1_400_000), self.count_ix_with(config, pairs)];
        let caller = self.funded();
        send(&mut self.svm, &ixs, &caller, &[&caller])
    }

    /// Anyone runs the daily count over the whole roster.
    fn count(&mut self) -> bool {
        let pairs = self.roster_pairs();
        self.count_with(self.config(), &pairs)
    }

    fn committed_bps(&self) -> u16 {
        self.config_state().last_count_bps
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

    fn transfer_ix(&self, mint: &Pubkey, program: &Pubkey, from: &Pubkey, to: &Pubkey, amount: u64) -> Instruction {
        spl_token_2022::instruction::transfer_checked(
            program,
            &ata(from, mint, program),
            mint,
            &ata(to, mint, program),
            from,
            &[],
            amount,
            DECIMALS,
        )
        .unwrap()
    }

    fn coin_transfer_ix(&self, from: &Pubkey, to: &Pubkey, amount: u64) -> Instruction {
        self.transfer_ix(&self.inst.coin_mint, &self.inst.coin_program, from, to, amount)
    }

    fn dividend_transfer_ix(&self, from: &Pubkey, to: &Pubkey, amount: u64) -> Instruction {
        self.transfer_ix(&self.inst.dividend_mint, &self.inst.dividend_program, from, to, amount)
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

    fn approve_amount_ix(&self, owner: &Pubkey, account: &Pubkey, delegate: &Pubkey, amount: u64) -> Instruction {
        spl_token_2022::instruction::approve(&self.inst.dividend_program, account, delegate, owner, &[], amount)
            .unwrap()
    }

    fn approve_ix(&self, owner: &Pubkey, account: &Pubkey) -> Instruction {
        self.approve_amount_ix(owner, account, &self.authority(), u64::MAX)
    }

    fn revoke(&mut self, owner: &Keypair) {
        let account = self.inst.dividend_account(&owner.pubkey());
        let ix = spl_token_2022::instruction::revoke(&self.inst.dividend_program, &account, &owner.pubkey(), &[])
            .unwrap();
        assert!(send(&mut self.svm, &[ix], owner, &[owner]));
    }

    fn register_accounts(&self, owner: &Pubkey, account: &Pubkey) -> endowment::accounts::RegisterLandlord {
        let config = self.config();
        endowment::accounts::RegisterLandlord {
            owner: *owner,
            config,
            authority: authority_pda(&config),
            landlord: landlord_pda(&config, owner),
            roster: roster_pda(&config),
            dividend_mint: self.inst.dividend_mint,
            dividend_account: *account,
            coin_mint: self.inst.coin_mint,
            coin_account: self.inst.coin_account(owner),
            dividend_token_program: self.inst.dividend_program,
            coin_token_program: self.inst.coin_program,
            system_program: system_program::ID,
            evict_landlord: None,
            evict_owner: None,
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

    /// Approve + register in one transaction, as the website sends it.
    fn register(&mut self, owner: &Keypair) -> bool {
        let account = self.inst.dividend_account(&owner.pubkey());
        let ixs = [self.approve_ix(&owner.pubkey(), &account), self.register_ix(&owner.pubkey(), &account)];
        send(&mut self.svm, &ixs, owner, &[owner])
    }

    /// A landlord holding `coin` of the coin that has delegated and registered.
    fn registered_holder(&mut self, dividend: u64, coin: u64) -> (Keypair, Pubkey) {
        let (owner, account) = self.new_landlord(dividend);
        self.mint_coin(&owner.pubkey(), coin);
        assert!(self.register(&owner));
        (owner, account)
    }

    fn registered_landlord(&mut self, starting: u64) -> (Keypair, Pubkey) {
        self.registered_holder(starting, 0)
    }

    fn resync_ix(&self, owner: &Pubkey) -> Instruction {
        let config = self.config();
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::ResyncBaseline {}.data(),
            endowment::accounts::ResyncBaseline {
                owner: *owner,
                config,
                landlord: landlord_pda(&config, owner),
                dividend_account: self.inst.dividend_account(owner),
            }
            .to_account_metas(None),
        )
    }

    fn deregister_with(&mut self, owner: &Keypair, config: Pubkey, landlord: Pubkey) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::DeregisterLandlord {}.data(),
            endowment::accounts::DeregisterLandlord {
                owner: owner.pubkey(),
                config,
                roster: roster_pda(&config),
                landlord,
            }
            .to_account_metas(None),
        );
        send(&mut self.svm, &[ix], owner, &[owner])
    }

    fn deregister(&mut self, owner: &Keypair) -> bool {
        let config = self.config();
        self.deregister_with(owner, config, landlord_pda(&config, &owner.pubkey()))
    }

    fn prune(&mut self, owner: &Pubkey) -> bool {
        let config = self.config();
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::PruneLandlord {}.data(),
            endowment::accounts::PruneLandlord {
                config,
                authority: authority_pda(&config),
                roster: roster_pda(&config),
                landlord: landlord_pda(&config, owner),
                owner: *owner,
                coin_mint: self.inst.coin_mint,
                dividend_account: self.inst.dividend_account(owner),
                coin_account: self.inst.coin_account(owner),
            }
            .to_account_metas(None),
        );
        let caller = self.funded();
        send(&mut self.svm, &[ix], &caller, &[&caller])
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
            dividend_token_program: self.inst.dividend_program,
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

// ---------------------------------------------------------------------------
// Creation.
// ---------------------------------------------------------------------------

#[test]
fn anyone_can_create_an_endowment_and_the_creator_is_admin_by_default() {
    let mut env = Env::new();
    env.create();
    let config = env.config_state();
    assert_eq!(config.version, 1);
    assert_eq!(config.creator, env.inst.creator.pubkey());
    assert_eq!(config.admin, env.inst.creator.pubkey());
    assert_eq!(config.guardian, env.guardian.pubkey());
    assert_eq!((config.coin_mint, config.dividend_mint, config.pool), (env.inst.coin_mint, env.inst.dividend_mint, env.inst.pool));
    assert_eq!(config.params, base_params());
    assert_eq!(config.pending.effective_at, 0);
    assert_eq!((config.active, config.retired, config.milestone_reached), (false, false, false));
    assert_eq!((config.contribution_cap, config.donation_bps), (CONTRIBUTION_CAP, 0));
    assert_eq!(config.buy_allowance, MAX_BUY_PER_TX);
    let roster = env.roster_state();
    assert_eq!((roster.version, roster.config, roster.entries.len()), (1, env.config(), 0));
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
    assert_err!(env.create_inst(&wrong, params(guardian, 0)), WrongPool);

    // A look-alike pool not owned by Raydium CPMM.
    let mut not_raydium = env.inst.clone();
    not_raydium.pool = fake_pool(&mut env.svm, [env.inst.coin_mint, env.inst.dividend_mint], Pubkey::new_unique());
    assert_err!(env.create_inst(&not_raydium, params(guardian, 0)), InvalidPoolData);

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
    assert_err!(env.create_with(with(&|p| p.contribution_cap = 0)), InvalidContributionCap);
    assert_err!(env.create_with(with(&|p| p.params.max_price_impact_bps = 301)), InvalidBuybackLimits);
    assert_err!(env.create_with(with(&|p| p.params.max_price_impact_bps = 9)), InvalidBuybackLimits);
    assert_err!(env.create_with(with(&|p| p.params.max_buy_per_tx = MAX_BUY_PER_DAY + 1)), InvalidBuybackLimits);
    assert_err!(env.create_with(with(&|p| p.params.min_buy_amount = MAX_BUY_PER_TX + 1)), InvalidBuyParams);
    assert_err!(env.create_with(with(&|p| p.params.tip_bps = 51)), InvalidBuyParams);
    assert_err!(env.create_with(with(&|p| p.params.min_buy_interval_secs = 59)), InvalidBuyParams);
    assert_err!(env.create_with(with(&|p| p.params.buy_bps = 10_001)), InvalidBuyParams);
    assert_err!(env.create_with(with(&|p| p.params.activate_bps = 5_001)), InvalidActivation);
    assert_err!(env.create_with(with(&|p| p.params.deactivate_bps = 3_001)), InvalidActivation);
    assert_err!(env.create_with(with(&|p| p.params.min_stake_bps = 501)), InvalidParams);
    // Donations are only possible in the flagship's dividend asset (PUMP); this
    // coin pays a different one.
    assert_err!(env.create_with(with(&|p| p.donation_bps = 10)), InvalidDonation);
    assert_err!(env.create_with(with(&|p| p.donation_bps = 15)), InvalidDonation);
    assert!(env.create_with(with(&|p| p.donation_bps = 0)));
}

#[test]
fn only_fixed_donation_rates_are_accepted_for_pump_paying_coins() {
    let mut env = Env::with_pool();
    let guardian = env.guardian.pubkey();
    let mut p = params(guardian, 15);
    assert_err!(env.create_with(p.clone()), InvalidDonation);
    p.donation_bps = 40;
    assert_err!(env.create_with(p.clone()), InvalidDonation);
    // A 50 bps tip plus a 30 bps donation is the most allowed.
    p.donation_bps = 30;
    p.params.tip_bps = 50;
    assert!(env.create_with(p));
    let config = env.config_state();
    assert_eq!((config.donation_bps, config.params.tip_bps), (30, 50));
}

// ---------------------------------------------------------------------------
// Landlords and sweeps.
// ---------------------------------------------------------------------------

#[test]
fn register_requires_a_full_delegation() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.new_landlord(100);

    let register = env.register_ix(&owner.pubkey(), &account);
    assert_err!(send(&mut env.svm, &[register.clone()], &owner, &[&owner]), NotDelegated);

    // A small approval isn't a delegation.
    let small = env.approve_amount_ix(&owner.pubkey(), &account, &env.authority(), 1_000);
    assert_err!(send(&mut env.svm, &[small, register.clone()], &owner, &[&owner]), NotDelegated);

    let approve = env.approve_ix(&owner.pubkey(), &account);
    assert!(send(&mut env.svm, &[approve, register], &owner, &[&owner]));
    let landlord = env.landlord_state(&owner.pubkey());
    assert_eq!((landlord.version, landlord.baseline, landlord.config), (1, 100, env.config()));
    let roster = env.roster_state();
    assert_eq!(roster.entries.len(), 1);
    assert_eq!(roster.entries[0].owner, owner.pubkey());
    assert!(!roster.entries[0].snapshot_valid);
}

#[test]
fn register_requires_the_minimum_stake() {
    let mut env = Env::new();
    env.create_active();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 1_000_000 * UNIT);

    // 10 bps of the supply, rounded up.
    let (small, _) = env.new_landlord(0);
    env.mint_coin(&small.pubkey(), 999 * UNIT);
    assert_err!(env.register(&small), StakeTooSmall);
    env.mint_coin(&small.pubkey(), 3 * UNIT);
    assert!(env.register(&small));
}

#[test]
fn sweeps_only_what_sits_above_the_baseline() {
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
    assert_eq!(env.config_state().total_swept, 250);

    // Nothing new: a sweep is a harmless no-op.
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 250);

    // The landlord spends 60 of their own. The baseline stays where it is.
    let burn = spl_token_2022::instruction::burn(&program, &account, &mint, &owner.pubkey(), &[], 60).unwrap();
    assert!(send(&mut env.svm, &[burn], &owner, &[&owner]));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 100);

    // Drops refill the landlord's own balance first.
    env.airdrop_dividend(&account, 70);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 100);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 260);
    let landlord = env.landlord_state(&owner.pubkey());
    assert_eq!((landlord.baseline, landlord.total_contributed), (100, 260));
}

#[test]
fn a_landlord_can_lower_its_own_baseline_with_resync() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(100);
    let program = env.inst.dividend_program;
    let mint = env.inst.dividend_mint;
    let burn = spl_token_2022::instruction::burn(&program, &account, &mint, &owner.pubkey(), &[], 60).unwrap();
    assert!(send(&mut env.svm, &[burn], &owner, &[&owner]));

    // Only the owner can resync.
    let stranger = env.funded();
    let mut forged = env.resync_ix(&owner.pubkey());
    forged.accounts[0] = AccountMeta::new_readonly(stranger.pubkey(), true);
    assert!(!send(&mut env.svm, &[forged], &stranger, &[&stranger]));
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 100);

    let resync = env.resync_ix(&owner.pubkey());
    assert!(send(&mut env.svm, &[resync], &owner, &[&owner]));
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 40);
    env.airdrop_dividend(&account, 10);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 10);
}

#[test]
fn revoking_delegation_stops_sweeps() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);

    // Opting out uses the token program directly, not the endowment.
    env.revoke(&owner);
    env.airdrop_dividend(&account, 500);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotDelegated);
    assert_eq!(token_balance(&env.svm, &account), 500);
}

#[test]
fn deregistering_leaves_the_roster_and_a_landlord_can_register_again_cleanly() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(100);
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));

    let lamports_before = env.svm.get_balance(&owner.pubkey()).unwrap();
    assert!(env.deregister(&owner));
    assert!(!exists(&env.svm, &landlord_pda(&env.config(), &owner.pubkey())));
    assert!(env.roster_state().entries.is_empty());
    assert!(env.svm.get_balance(&owner.pubkey()).unwrap() > lamports_before);

    // Coming back starts fresh: the new baseline is whatever the landlord holds now.
    env.airdrop_dividend(&account, 400);
    assert!(env.register(&owner));
    let landlord = env.landlord_state(&owner.pubkey());
    assert_eq!((landlord.baseline, landlord.total_contributed), (500, 0));
    assert_eq!(env.roster_state().entries.len(), 1);
}

#[test]
fn guardian_pause_blocks_sweeps_and_expires() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, 50);

    let stranger = env.funded();
    assert_err!(env.pause(&stranger), NotGuardian);

    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    assert_err!(env.sweep(&owner.pubkey(), &account), Paused);

    env.warp(MAX_PAUSE_SECONDS);
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
    assert_err!(env.unpause(&guardian), NotAdmin);
    env.airdrop_dividend(&account, 5);
    assert_err!(env.sweep(&owner.pubkey(), &account), Paused);

    assert!(env.unpause(&admin));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 5);
    // The cooldown runs from the early unpause.
    assert_err!(env.pause(&guardian), PauseCooldown);
    env.warp(PAUSE_COOLDOWN_SECONDS);
    assert!(env.pause(&guardian));
}

#[test]
fn leaving_is_never_blocked() {
    let mut env = Env::new();
    env.create_active();
    let (owner, _) = env.registered_landlord(0);
    let guardian = env.guardian.insecure_clone();
    let admin = env.admin();
    assert!(env.pause(&guardian));
    assert!(env.retire(&admin));
    env.revoke(&owner);
    assert!(env.deregister(&owner));
}

#[test]
fn admin_rotates_the_guardian() {
    let mut env = Env::new();
    env.create();
    let old_guardian = env.guardian.insecure_clone();
    let admin = env.admin();
    let new_guardian = env.funded();

    let stranger = env.funded();
    assert_err!(env.set_guardian(&stranger, stranger.pubkey()), NotAdmin);
    assert_err!(env.set_guardian(&old_guardian, new_guardian.pubkey()), NotAdmin);

    assert!(env.set_guardian(&admin, new_guardian.pubkey()));
    assert_eq!(env.config_state().guardian, new_guardian.pubkey());
    assert_err!(env.pause(&old_guardian), NotGuardian);
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
    assert_err!(env.propose_admin(&stranger, stranger.pubkey()), NotAdmin);
    assert_err!(env.accept_admin(&new_admin), NotPendingAdmin);
    assert!(env.propose_admin(&old_admin, new_admin.pubkey()));
    assert_eq!(env.config_state().admin, old_admin.pubkey());
    assert_err!(env.accept_admin(&stranger), NotPendingAdmin);

    // Proposing the default key cancels.
    assert!(env.propose_admin(&old_admin, Pubkey::default()));
    assert_err!(env.accept_admin(&new_admin), NotPendingAdmin);

    assert!(env.propose_admin(&old_admin, new_admin.pubkey()));
    assert!(env.accept_admin(&new_admin));
    let config = env.config_state();
    assert_eq!((config.admin, config.pending_admin), (new_admin.pubkey(), Pubkey::default()));

    // The old admin has lost its rights; the new one has them.
    assert_err!(env.set_guardian(&old_admin, old_admin.pubkey()), NotAdmin);
    assert!(env.set_guardian(&new_admin, new_admin.pubkey()));
}

// ---------------------------------------------------------------------------
// Parameters: propose, wait 72 hours, apply.
// ---------------------------------------------------------------------------

#[test]
fn parameter_changes_wait_out_the_timelock() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let stranger = env.funded();
    let mut next = base_params();
    next.max_buy_per_tx = UNIT;
    next.max_buy_per_day = 10 * UNIT;
    next.max_price_impact_bps = 50;
    next.buy_bps = 6_000;
    next.min_buy_interval_secs = 900;
    next.tip_bps = 50;

    assert_err!(env.propose(&stranger, next), NotAdmin);
    assert_err!(env.apply_params(), NoPendingParams);
    assert!(env.propose(&admin, next));
    let pending = env.config_state().pending;
    assert_eq!((pending.params, pending.effective_at), (next, env.now() + PARAM_TIMELOCK_SECONDS));

    // Not yet.
    assert_err!(env.apply_params(), TimelockNotElapsed);
    env.warp(PARAM_TIMELOCK_SECONDS - 1);
    assert_err!(env.apply_params(), TimelockNotElapsed);
    assert_eq!(env.config_state().params, base_params());

    // Anyone applies it once the time is up.
    env.warp(1);
    assert!(env.apply_params());
    let config = env.config_state();
    assert_eq!(config.params, next);
    assert_eq!(config.pending.effective_at, 0);
    // The allowance never exceeds the new per-transaction cap.
    assert_eq!(config.buy_allowance, UNIT);
    assert_err!(env.apply_params(), NoPendingParams);
}

#[test]
fn a_pending_change_can_be_cancelled_or_replaced() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let stranger = env.funded();
    let mut next = base_params();
    next.tip_bps = 10;
    assert!(env.propose(&admin, next));
    assert_err!(env.cancel_params(&stranger), NotAdmin);
    assert!(env.cancel_params(&admin));
    assert_err!(env.cancel_params(&admin), NoPendingParams);
    env.warp(PARAM_TIMELOCK_SECONDS);
    assert_err!(env.apply_params(), NoPendingParams);

    // A new proposal restarts the clock.
    assert!(env.propose(&admin, next));
    env.warp(PARAM_TIMELOCK_SECONDS - 10);
    next.tip_bps = 20;
    assert!(env.propose(&admin, next));
    env.warp(10);
    assert_err!(env.apply_params(), TimelockNotElapsed);
    env.warp(PARAM_TIMELOCK_SECONDS);
    assert!(env.apply_params());
    assert_eq!(env.config_state().params.tip_bps, 20);
}

#[test]
fn proposals_are_bounded() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let with = |f: &dyn Fn(&mut Params)| {
        let mut p = base_params();
        f(&mut p);
        p
    };
    assert_err!(env.propose(&admin, with(&|p| p.max_price_impact_bps = 301)), InvalidBuybackLimits);
    assert_err!(env.propose(&admin, with(&|p| p.max_buy_per_tx = 0)), InvalidBuybackLimits);
    assert_err!(env.propose(&admin, with(&|p| p.max_buy_per_day = p.max_buy_per_tx - 1)), InvalidBuybackLimits);
    assert_err!(env.propose(&admin, with(&|p| p.buy_bps = 10_001)), InvalidBuyParams);
    assert_err!(env.propose(&admin, with(&|p| p.min_buy_interval_secs = 59)), InvalidBuyParams);
    assert_err!(env.propose(&admin, with(&|p| p.min_buy_interval_secs = DAY + 1)), InvalidBuyParams);
    assert_err!(env.propose(&admin, with(&|p| p.tip_bps = 51)), InvalidBuyParams);
    assert_err!(env.propose(&admin, with(&|p| p.activate_bps = 5_001)), InvalidActivation);
    assert_err!(env.propose(&admin, with(&|p| {
        p.activate_bps = 2_000;
        p.deactivate_bps = 2_500;
    })), InvalidActivation);
    assert_err!(env.propose(&admin, with(&|p| p.min_stake_bps = 501)), InvalidParams);
    assert!(env.propose(&admin, with(&|p| p.min_stake_bps = 500)));
}

#[test]
fn new_activation_thresholds_apply_to_the_last_count_at_once() {
    let mut env = Env::new();
    env.create();
    assert!(!env.config_state().active);
    // Zero turns sweeps on at once, for a founders-only test window.
    env.change_params(|p| {
        p.activate_bps = 0;
        p.deactivate_bps = 0;
    });
    assert!(env.config_state().active);
    env.change_params(|p| {
        p.activate_bps = 4_000;
        p.deactivate_bps = 3_500;
    });
    assert!(!env.config_state().active);
}

#[test]
fn renounced_admin_freezes_every_parameter_and_removes_the_guardian() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let stranger = env.funded();
    let guardian = env.guardian.insecure_clone();
    assert_err!(env.renounce(&stranger), NotAdmin);
    assert!(env.propose_admin(&admin, stranger.pubkey()));
    assert!(env.propose(&admin, base_params()));
    assert!(env.renounce(&admin));

    let config = env.config_state();
    assert_eq!(
        (config.admin, config.pending_admin, config.guardian, config.pending.effective_at),
        (Pubkey::default(), Pubkey::default(), Pubkey::default(), 0)
    );
    assert_err!(env.accept_admin(&stranger), NotPendingAdmin);
    assert_err!(env.propose(&admin, base_params()), NotAdmin);
    env.warp(PARAM_TIMELOCK_SECONDS);
    assert_err!(env.apply_params(), NoPendingParams);
    assert_err!(env.retire(&admin), NotAdmin);
    assert_err!(env.pause(&guardian), NotGuardian);
}

// ---------------------------------------------------------------------------
// Buybacks run against the real Raydium CPMM program and mainnet pool state.
// ---------------------------------------------------------------------------

fn pool_env_with(dividend_in_vault: u64, f: impl FnOnce(&mut CreateParams)) -> Env {
    let mut env = Env::with_pool();
    env.create_custom(f);
    let vault = env.dividend_vault();
    env.set_balance(&vault, dividend_in_vault);
    let authority = env.authority();
    env.create_lp_vault(&authority);
    env
}

fn funded_pool_env(dividend_in_vault: u64) -> Env {
    pool_env_with(dividend_in_vault, |_| {})
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
    assert_eq!(config.total_coin_retained, received);
    assert_eq!((config.total_tips, config.total_donated), (tip, 0));
    assert_eq!((config.buy_allowance, config.last_buy_at), (0, env.now()));
    assert_eq!((config.total_lp_tokens, config.milestone_reached), (0, false));
}

#[test]
fn buyback_spends_what_the_vault_holds_when_below_the_cap() {
    let mut env = funded_pool_env(1_002_500_000); // 1,000 + a 25 bps tip
    assert!(env.buy());
    assert_eq!(env.config_state().total_dividend_spent, 1_000 * UNIT);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);

    // An empty vault has nothing to buy.
    env.warp(DAY);
    assert_err!(env.buy(), NothingToBuy);
}

#[test]
fn buyback_enforces_spacing() {
    let mut env = funded_pool_env(100_000 * UNIT);
    assert!(env.buy());
    assert_err!(env.buy(), BuyTooSoon);
    env.warp(INTERVAL - 1);
    assert_err!(env.buy(), BuyTooSoon);
    env.warp(1);
    assert!(env.buy());
}

#[test]
fn buyback_is_blocked_while_paused() {
    let mut env = funded_pool_env(10_000 * UNIT);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    assert_err!(env.buy(), Paused);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 10_000 * UNIT);
}

#[test]
fn buyback_does_not_need_landlord_activation() {
    let mut env = funded_pool_env(10_000 * UNIT);
    assert!(!env.config_state().active);
    assert!(env.buy());
}

#[test]
fn buyback_rejects_a_fill_below_the_callers_minimum() {
    let mut env = funded_pool_env(10_000 * UNIT);
    assert!(!env.buyback(u64::MAX).0);
    assert_eq!(token_balance(&env.svm, &env.coin_vault()), 0);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 10_000 * UNIT);
}

#[test]
fn buyback_rejects_accounts_from_other_pools_and_foreign_tip_accounts() {
    let mut env = funded_pool_env(10_000 * UNIT);
    let caller = env.cranker();

    let mut wrong_pool = env.buyback_accounts(&caller.pubkey());
    wrong_pool.pool_state = fixtures::key(fixtures::AMM_CONFIG);
    assert_err!(env.buyback_with(&caller, wrong_pool, 1), WrongPool);

    // The pool's two vaults swapped: the dividend would flow the wrong way.
    let mut swapped = env.buyback_accounts(&caller.pubkey());
    std::mem::swap(&mut swapped.pool_dividend_vault, &mut swapped.pool_coin_vault);
    assert_err!(env.buyback_with(&caller, swapped, 1), WrongPool);

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
fn buyback_fails_closed_on_a_high_pool_fee() {
    let mut env = funded_pool_env(10_000 * UNIT);
    // 2.5% trade fee, above the 2% ceiling.
    env.poke(&fixtures::key(fixtures::AMM_CONFIG), |d| {
        d[AMM_TRADE_FEE_RATE..AMM_TRADE_FEE_RATE + 8].copy_from_slice(&25_000u64.to_le_bytes())
    });
    assert_err!(env.buy(), FeeTooHigh);
}

#[test]
fn buyback_fails_closed_when_the_pool_claims_more_fees_than_it_holds() {
    let mut env = funded_pool_env(10_000 * UNIT);
    let pool = env.inst.pool;
    env.poke(&pool, |d| {
        d[POOL_PROTOCOL_FEES_0..POOL_PROTOCOL_FEES_0 + 8].copy_from_slice(&(u64::MAX / 4).to_le_bytes())
    });
    assert_err!(env.buy(), InvalidPoolData);
}

#[test]
fn buyback_fails_closed_when_the_pool_has_swaps_disabled() {
    let mut env = funded_pool_env(10_000 * UNIT);
    let pool = env.inst.pool;
    env.poke(&pool, |d| d[POOL_STATUS] |= 4);
    assert_err!(env.buy(), PoolSwapDisabled);
}

#[test]
fn buyback_needs_a_full_twap_window_of_price_history() {
    let mut env = funded_pool_env(10_000 * UNIT);
    // Every observation and the last update are "now": no history a window old.
    let now = env.now() as u64;
    env.poke(&fixtures::key(fixtures::OBSERVATION), |d| {
        for i in 0..100 {
            let at = OBSERVATIONS + 40 * i;
            d[at..at + 8].copy_from_slice(&now.to_le_bytes());
        }
        d[OBSERVATION_LAST_UPDATE..OBSERVATION_LAST_UPDATE + 8].copy_from_slice(&now.to_le_bytes());
    });
    assert_err!(env.buy(), TwapUnavailable);
}

#[test]
fn the_buy_allowance_refills_smoothly_and_caps_any_24_hours() {
    let mut env = funded_pool_env(100_000 * UNIT);
    assert!(env.buy());
    assert_eq!(env.config_state().total_dividend_spent, MAX_BUY_PER_TX);

    // One interval later, only what refilled in that time can be spent.
    env.warp(INTERVAL);
    assert!(env.buy());
    let refill = (MAX_BUY_PER_DAY as u128 * INTERVAL as u128 / DAY as u128) as u64;
    let second = env.config_state().total_dividend_spent - MAX_BUY_PER_TX;
    assert!(second.abs_diff(refill) <= 1, "spent {second}, refill {refill}");

    // Buying every hour for the rest of the day never exceeds a day's refill plus one full transaction.
    for _ in 0..23 {
        env.warp(60 * 60);
        assert!(env.buy());
    }
    let total = env.config_state().total_dividend_spent;
    assert!(total <= MAX_BUY_PER_DAY + MAX_BUY_PER_TX, "{total}");
    assert!(total > MAX_BUY_PER_DAY, "{total}");
}

// Donations to the flagship endowment.

#[test]
fn a_donating_endowment_sends_its_share_to_the_flagship_vault() {
    let mut env = pool_env_with(50_000 * UNIT, |p| p.donation_bps = 20);
    env.create_flagship_vault();

    // The donation can only go to the flagship's vault.
    let caller = env.cranker();
    let mut elsewhere = env.buyback_accounts(&caller.pubkey());
    elsewhere.flagship_dividend_vault = env.inst.dividend_account(&caller.pubkey());
    assert_err!(env.buyback_with(&caller, elsewhere, 1), WrongFlagshipVault);

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
    // 1,000 + 25 bps tip + 30 bps donation.
    let mut env = pool_env_with(1_005_500_000, |p| p.donation_bps = 30);
    env.create_flagship_vault();
    assert!(env.buy());
    assert_eq!(env.config_state().total_dividend_spent, 1_000 * UNIT);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);
    assert_eq!(token_balance(&env.svm, &env.flagship_vault()), 3 * UNIT);
}

// ---------------------------------------------------------------------------
// The milestone: after the endowment has bought `contribution_cap` of its coin,
// part of each buyback becomes locked liquidity. Contributions keep flowing.
// ---------------------------------------------------------------------------

/// An endowment whose milestone is reached on its first buy, with a 50/50 split after.
fn milestone_env() -> Env {
    let mut env = pool_env_with(100_000 * UNIT, |p| {
        p.contribution_cap = 1;
        p.params.buy_bps = 5_000;
    });
    assert!(env.buy());
    let config = env.config_state();
    assert!(config.milestone_reached);
    // The first buy was all buying.
    assert_eq!((config.total_lp_tokens, config.total_liquidity_dividend), (0, 0));
    env.warp(DAY);
    env
}

#[test]
fn after_the_milestone_part_of_each_buy_becomes_locked_liquidity() {
    let mut env = milestone_env();
    let coin_before = token_balance(&env.svm, &env.coin_vault());
    let lp_supply_before = env.lp_supply();
    let before = env.config_state();
    assert!(env.buy());

    let config = env.config_state();
    let lp = token_balance(&env.svm, &env.lp_vault());
    assert!(lp > 0);
    assert_eq!(config.total_lp_tokens, lp);
    assert_eq!(env.lp_supply(), lp_supply_before + lp);
    // Half the buy went to liquidity: a bit under a quarter swapped, the rest deposited as the dividend.
    let spent = config.total_dividend_spent - before.total_dividend_spent;
    assert!(config.total_liquidity_dividend > 0 && config.total_liquidity_dividend <= spent / 4 + 1);
    assert!(config.total_liquidity_coin > 0);
    // The coin vault never shrinks.
    let coin_after = token_balance(&env.svm, &env.coin_vault());
    assert!(coin_after > coin_before);
    assert_eq!(config.total_coin_retained, coin_after);
}

#[test]
fn liquidity_can_only_land_in_the_authoritys_lp_account() {
    let mut env = milestone_env();
    let caller = env.cranker();
    // The caller's own LP account instead of the endowment's.
    let create = create_associated_token_account_idempotent(
        &caller.pubkey(),
        &caller.pubkey(),
        &fixtures::key(fixtures::LP_MINT),
        &TOKEN,
    );
    assert!(send(&mut env.svm, &[create], &caller, &[&caller]));
    let mut accounts = env.buyback_accounts(&caller.pubkey());
    accounts.lp_vault = ata(&caller.pubkey(), &fixtures::key(fixtures::LP_MINT), &TOKEN);
    assert_err!(env.buyback_with(&caller, accounts, 1), WrongPool);

    let accounts = env.buyback_accounts(&caller.pubkey());
    assert!(env.buyback_with(&caller, accounts, 1));
    assert!(token_balance(&env.svm, &env.lp_vault()) > 0);
}

#[test]
fn liquidity_waits_while_the_pool_has_deposits_disabled() {
    let mut env = milestone_env();
    let pool = env.inst.pool;
    env.poke(&pool, |d| d[POOL_STATUS] |= 1);
    let before = env.config_state();
    assert!(env.buy());
    let config = env.config_state();
    // Only the buying half was spent; the liquidity half stays in the vault.
    assert_eq!(config.total_lp_tokens, 0);
    assert_eq!(config.total_liquidity_dividend, 0);
    assert_eq!(config.total_dividend_spent - before.total_dividend_spent, MAX_BUY_PER_TX / 2);

    env.poke(&pool, |d| d[POOL_STATUS] &= !1);
    env.warp(DAY);
    assert!(env.buy());
    assert!(env.config_state().total_lp_tokens > 0);
}

#[test]
fn sweeps_keep_flowing_after_the_milestone() {
    let mut env = pool_env_with(100_000 * UNIT, |p| {
        p.contribution_cap = 1;
        p.params.activate_bps = 0;
        p.params.deactivate_bps = 0;
        p.params.min_stake_bps = 0;
    });
    let (owner, account) = env.registered_landlord(0);
    assert!(env.buy());
    assert!(env.config_state().milestone_reached);

    env.set_balance(&account, 700 * UNIT);
    let vault_before = token_balance(&env.svm, &env.dividend_vault());
    assert!(env.sweep(&owner.pubkey(), &account));
    assert!(token_balance(&env.svm, &env.dividend_vault()) > vault_before);
    assert_eq!(token_balance(&env.svm, &account), 0);
    assert!(env.config_state().total_swept > 0);
}

#[test]
fn retire_stops_sweeps_and_registrations_for_good_and_is_admin_only() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    let admin = env.admin();
    let stranger = env.funded();

    assert_err!(env.retire(&stranger), NotAdmin);
    assert!(env.retire(&admin));
    assert!(env.config_state().retired);
    env.airdrop_dividend(&account, 10);
    assert_err!(env.sweep(&owner.pubkey(), &account), Retired);
    let (newcomer, _) = env.new_landlord(0);
    assert_err!(env.register(&newcomer), Retired);
    // Retiring again is a no-op; there is no way back.
    assert!(env.retire(&admin));
    assert!(env.config_state().retired);
}

#[test]
fn the_admin_can_renounce_once_retired_even_with_test_thresholds() {
    let mut env = Env::new();
    env.create_active();
    let admin = env.admin();
    assert_err!(env.renounce(&admin), RenounceThresholds);
    assert!(env.retire(&admin));
    assert!(env.renounce(&admin));
}

// ---------------------------------------------------------------------------
// Isolation between endowments.
// ---------------------------------------------------------------------------

#[test]
fn landlords_of_one_endowment_cant_be_registered_swept_counted_or_removed_through_another() {
    let mut env = Env::new();
    env.create_active();
    let a = env.inst.clone();
    let b = env.second_instance();
    let guardian = env.guardian.pubkey();
    let mut p = params(guardian, 0);
    p.params.activate_bps = 0;
    p.params.deactivate_bps = 0;
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
    assert_err!(send(&mut env.svm, &[approve_a, register_b], &other, &[&other]), NotDelegated);
    // Nor can A's authority be passed in B's place.
    let mut mixed = env.register_accounts(&other.pubkey(), &other_account);
    mixed.authority = a.authority();
    let ix = env.register_ix_with(mixed);
    let approve_a = env.approve_amount_ix(&other.pubkey(), &other_account, &a.authority(), u64::MAX);
    assert!(!send(&mut env.svm, &[approve_a, ix], &other, &[&other]));
    // Delegating to B's own authority is what registering with B takes.
    assert!(env.register(&other));
    assert_eq!(env.landlord_state(&other.pubkey()).config, b.config());

    // Sweeping A's landlord through B's config, authority or vault fails.
    env.inst = a.clone();
    let mut via_b = env.sweep_accounts(&owner.pubkey(), &account);
    via_b.config = b.config();
    via_b.authority = b.authority();
    via_b.dividend_vault = b.dividend_vault();
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

    // B's count can't read A's landlord in place of its own.
    let a_pair = (a.coin_account(&owner.pubkey()), a.dividend_account(&owner.pubkey()));
    assert_err!(env.count_with(b.config(), &[a_pair]), InvalidCountAccount);

    // A's landlord can't be deregistered through B.
    let landlord_a = landlord_pda(&a.config(), &owner.pubkey());
    assert!(!env.deregister_with(&owner, b.config(), landlord_a));
    assert_eq!(env.roster_state().entries.len(), 1);
    assert!(env.deregister(&owner));
    assert!(env.roster_state().entries.is_empty());
    env.inst = b.clone();
    assert_eq!(env.roster_state().entries.len(), 1);
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
    // Nor deposit A's liquidity into B's LP account.
    let mut b_lp = env.buyback_accounts_for(&a, &caller.pubkey());
    b_lp.lp_vault = Env::lp_vault_for(&b.authority());
    assert_err!(env.buyback_with(&caller, b_lp, 1), WrongPool);
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
    let (owner, account) = env.registered_holder(10, 400);
    env.airdrop_dividend(&account, 90);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 90);
    assert_eq!(token_balance(&env.svm, &account), 10);

    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 600);
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 4_000);
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

// ---------------------------------------------------------------------------
// Activation: the daily commitment count.
// ---------------------------------------------------------------------------

/// Three landlords holding 10%, 15% and 5% of a 1,000,000 coin supply; the rest
/// sits with someone who isn't a landlord.
fn counted_env() -> (Env, Vec<Keypair>) {
    let mut env = Env::new();
    env.create();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 700_000 * UNIT);
    let mut owners = vec![];
    for share in [100_000, 150_000, 50_000] {
        let (owner, _) = env.registered_holder(0, share * UNIT);
        owners.push(owner);
    }
    (env, owners)
}

#[test]
fn sweeps_wait_for_activation() {
    let mut env = Env::new();
    env.create();
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, 50);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotActive);
    assert_eq!(token_balance(&env.svm, &account), 50);
}

#[test]
fn count_switches_sweeps_on_at_30_percent_with_hysteresis() {
    let (mut env, owners) = counted_env();

    // The first count only records what each landlord holds.
    assert!(env.count());
    let config = env.config_state();
    assert_eq!((config.last_count_bps, config.active, config.last_count_at), (0, false, env.now()));
    assert!(env.roster_state().entries.iter().all(|e| e.snapshot_valid));

    // Held across a full day: 30% committed, on.
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    let config = env.config_state();
    assert_eq!((config.last_count_bps, config.active, config.last_committed), (3_000, true, 300_000 * UNIT));

    // Down to about 28%: between the lines, so it stays on.
    env.burn_coin(&owners[0], 30_000 * UNIT);
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    let config = env.config_state();
    assert_eq!((config.last_count_bps, config.active), (2_783, true));

    // Down to about 24%: below 25%, off.
    env.burn_coin(&owners[1], 50_000 * UNIT);
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(!env.config_state().active);

    // And sweeps stop.
    let owner = owners[2].insecure_clone();
    let account = env.inst.dividend_account(&owner.pubkey());
    env.airdrop_dividend(&account, 10);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotActive);
}

#[test]
fn count_runs_at_most_once_a_day_and_not_while_paused() {
    let (mut env, _) = counted_env();
    assert!(env.count());
    assert_err!(env.count(), CountTooSoon);
    env.warp(COUNT_INTERVAL_SECS - 1);
    assert_err!(env.count(), CountTooSoon);
    env.warp(1);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    assert_err!(env.count(), Paused);
    env.warp(MAX_PAUSE_SECONDS);
    assert!(env.count());
}

#[test]
fn count_must_list_every_landlord_once_in_roster_order() {
    let (mut env, owners) = counted_env();
    let config = env.config();
    let pairs = env.roster_pairs();

    // Missing one.
    assert_err!(env.count_with(config, &pairs[..2]), InvalidCountAccount);
    // One twice in place of another.
    assert_err!(env.count_with(config, &[pairs[0], pairs[0], pairs[2]]), InvalidCountAccount);
    // Out of order.
    assert_err!(env.count_with(config, &[pairs[1], pairs[0], pairs[2]]), InvalidCountAccount);
    // Someone else's coin account in a landlord's place.
    let stranger = env.new_landlord(0).0;
    let forged = (env.inst.coin_account(&stranger.pubkey()), pairs[0].1);
    assert_err!(env.count_with(config, &[forged, pairs[1], pairs[2]]), InvalidCountAccount);
    // A coin account posing as a dividend account (or vice versa).
    let swapped = (pairs[0].1, pairs[0].0);
    assert_err!(env.count_with(config, &[swapped, pairs[1], pairs[2]]), InvalidCountAccount);
    let _ = owners;
    assert!(env.count_with(config, &pairs));
}

#[test]
fn new_landlords_count_from_their_second_count() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);

    // A new landlord joins: it sits this count out.
    let (late, _) = env.registered_holder(0, 50_000 * UNIT);
    assert!(env.count());
    // 300,000 of 1,050,000.
    assert_eq!(env.committed_bps(), 2_857);

    // A landlord leaves: the count simply no longer includes it.
    assert!(env.deregister(&owners[0]));
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    // owners[1], owners[2] and late: 250,000 of 1,050,000.
    assert_eq!(env.committed_bps(), 2_380);
    assert_eq!(env.config_state().last_committed, 250_000 * UNIT);
    let _ = late;
}

#[test]
fn a_closed_coin_or_dividend_account_counts_as_zero() {
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

    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    // 250,000 of 950,000.
    assert_eq!(env.committed_bps(), 2_631);

    // Closing the dividend account (empty) ends the delegation too.
    let dividend = env.inst.dividend_account(&owners[1].pubkey());
    let close = spl_token_2022::instruction::close_account(
        &env.inst.dividend_program,
        &dividend,
        &owners[1].pubkey(),
        &owners[1].pubkey(),
        &[],
    )
    .unwrap();
    assert!(send(&mut env.svm, &[close], &owners[1], &[&owners[1]]));
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 1_052);
}

#[test]
fn landlords_below_the_minimum_stake_dont_count() {
    let mut env = Env::new();
    env.create();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 900_000 * UNIT);
    let (big, _) = env.registered_holder(0, 98_000 * UNIT);
    let (small, _) = env.registered_holder(0, 2_000 * UNIT);
    assert!(env.count());
    // `small` drops below 10 bps of the 1,000,000 supply.
    env.burn_coin(&small, 1_500 * UNIT);
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    // Only `big`: 98,000 of 998,500.
    assert_eq!(env.config_state().last_committed, 98_000 * UNIT);
    let _ = big;
}

// ---------------------------------------------------------------------------
// The roster: at most MAX_LANDLORDS, the smallest replaceable by a larger one,
// and landlords that no longer qualify prunable by anyone.
// ---------------------------------------------------------------------------

/// A full roster; landlord i holds (i + 10) × 1,000 of the coin.
fn full_roster_env() -> (Env, Vec<Keypair>) {
    let mut env = Env::new();
    env.create();
    let mut landlords = vec![];
    for i in 0..MAX_LANDLORDS as u64 {
        let (owner, _) = env.new_landlord(0);
        env.mint_coin(&owner.pubkey(), (i + 10) * 1_000 * UNIT);
        landlords.push(owner);
    }
    for owner in &landlords {
        assert!(env.register(owner));
    }
    assert_eq!(env.roster_state().entries.len(), MAX_LANDLORDS);
    (env, landlords)
}

#[test]
fn a_full_roster_admits_a_newcomer_only_in_place_of_a_smaller_landlord() {
    let (mut env, landlords) = full_roster_env();
    let config = env.config();

    let (newcomer, account) = env.new_landlord(0);
    env.mint_coin(&newcomer.pubkey(), 50_000 * UNIT);
    // Full, and no one named to replace.
    assert_err!(env.register(&newcomer), RosterFull);

    let register_evicting = |env: &Env, victim: &Pubkey, owner: &Keypair| {
        let mut accounts = env.register_accounts(&owner.pubkey(), &env.inst.dividend_account(&owner.pubkey()));
        accounts.evict_landlord = Some(landlord_pda(&config, victim));
        accounts.evict_owner = Some(*victim);
        [env.approve_ix(&owner.pubkey(), &env.inst.dividend_account(&owner.pubkey())), env.register_ix_with(accounts)]
    };

    // Not the smallest.
    let ixs = register_evicting(&env, &landlords[1].pubkey(), &newcomer);
    assert_err!(send(&mut env.svm, &ixs, &newcomer, &[&newcomer]), InvalidEviction);

    // The smallest, but the newcomer isn't larger.
    let (minnow, _) = env.new_landlord(0);
    env.mint_coin(&minnow.pubkey(), 9_000 * UNIT);
    let ixs = register_evicting(&env, &landlords[0].pubkey(), &minnow);
    assert_err!(send(&mut env.svm, &ixs, &minnow, &[&minnow]), InvalidEviction);

    // The smallest, replaced by someone larger; its rent goes back to its owner.
    let victim = landlords[0].pubkey();
    let victim_lamports = env.svm.get_balance(&victim).unwrap();
    let ixs = register_evicting(&env, &victim, &newcomer);
    assert!(send(&mut env.svm, &ixs, &newcomer, &[&newcomer]));
    assert!(!exists(&env.svm, &landlord_pda(&config, &victim)));
    assert!(env.svm.get_balance(&victim).unwrap() > victim_lamports);
    let roster = env.roster_state();
    assert_eq!(roster.entries.len(), MAX_LANDLORDS);
    assert!(roster.position(&victim).is_none());
    assert!(roster.position(&newcomer.pubkey()).is_some());
    assert_eq!(env.landlord_state(&newcomer.pubkey()).dividend_account, account);
}

#[test]
fn landlords_that_no_longer_qualify_can_be_pruned_by_anyone() {
    let (mut env, owners) = counted_env();
    // Still delegated and staked.
    assert_err!(env.prune(&owners[0].pubkey()), NotPrunable);

    // Revoked: pruned, rent back to the owner.
    env.revoke(&owners[0]);
    let lamports = env.svm.get_balance(&owners[0].pubkey()).unwrap();
    assert!(env.prune(&owners[0].pubkey()));
    assert!(!exists(&env.svm, &landlord_pda(&env.config(), &owners[0].pubkey())));
    assert!(env.svm.get_balance(&owners[0].pubkey()).unwrap() > lamports);
    assert_eq!(env.roster_state().entries.len(), 2);

    // Below the minimum stake: pruned.
    env.burn_coin(&owners[2], 49_500 * UNIT);
    assert!(env.prune(&owners[2].pubkey()));
    assert_eq!(env.roster_state().entries.len(), 1);
    assert!(env.count());
}

#[test]
fn the_count_of_a_full_roster_fits_one_transaction() {
    let (mut env, _) = full_roster_env();
    let ix = env.count_ix();
    // 2 accounts per landlord + config, roster and coin mint.
    assert_eq!(ix.accounts.len(), 2 * MAX_LANDLORDS + 3);
    // With the payer, the endowment program and the compute budget program: within 64 locks.
    assert!(ix.accounts.len() + 3 <= 64);

    assert!(env.count());
    let first = last_cu();
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    let steady = last_cu();
    println!("count_commitment with {MAX_LANDLORDS} landlords: {first} CU (first count), {steady} CU (steady state)");
    assert!(env.config_state().active);
    assert!(first < 1_400_000 && steady < 1_400_000);
}

// ---------------------------------------------------------------------------
// Regressions: the round-1 audit's exploits, which must now fail.
// ---------------------------------------------------------------------------

/// Sybil landlords a, b, c; the attacker holds 10% of supply and some dust in each.
fn sybil_env() -> (Env, Vec<Keypair>) {
    let mut env = Env::new();
    env.create();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 896_000 * UNIT);
    let mut sybils = vec![];
    for coin in [100_000, 2_000, 2_000] {
        let (s, _) = env.registered_holder(0, coin * UNIT);
        sybils.push(s);
    }
    (env, sybils)
}

#[test]
fn regression_h01_moving_coin_between_landlords_across_counts_counts_nothing() {
    let (mut env, s) = sybil_env();
    let (a, b, c) = (s[0].pubkey(), s[1].pubkey(), s[2].pubkey());
    assert!(env.count());
    // Before each count, the attacker moves the 10% to the next landlord.
    for (from, to, signer) in [(a, b, &s[0]), (b, c, &s[1]), (c, a, &s[2])] {
        let ix = env.coin_transfer_ix(&from, &to, 100_000 * UNIT);
        assert!(send(&mut env.svm, &[ix], signer, &[signer]));
        env.warp(COUNT_INTERVAL_SECS);
        assert!(env.count());
        // Only coin held across a whole interval counts: the dust, never the 10%.
        assert!(env.committed_bps() < 100, "{}", env.committed_bps());
        assert!(!env.config_state().active);
    }
}

#[test]
fn regression_h01_the_same_coin_cant_be_counted_twice_in_one_transaction() {
    let (mut env, s) = sybil_env();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    // count, move a → b, count again: the second count is refused.
    let ixs = [
        compute_limit_ix(1_400_000),
        env.count_ix(),
        env.coin_transfer_ix(&s[0].pubkey(), &s[1].pubkey(), 100_000 * UNIT),
        env.count_ix(),
    ];
    assert_err!(send(&mut env.svm, &ixs, &s[0], &[&s[0]]), CountTooSoon);
    // One honest count: 104,000 of 1,000,000.
    assert!(env.count());
    assert_eq!(env.committed_bps(), 1_040);
}

#[test]
fn regression_m01_a_revoked_landlord_counts_nothing() {
    let mut env = Env::new();
    env.create();
    let (owner, account) = env.registered_holder(0, 1_000 * UNIT);
    env.revoke(&owner);
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 0);
    assert!(!env.config_state().active);

    // A token approval of a small amount doesn't count either.
    let small = env.approve_amount_ix(&owner.pubkey(), &account, &env.authority(), 1);
    assert!(send(&mut env.svm, &[small], &owner, &[&owner]));
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 0);

    // Re-delegating fully restores it.
    let full = env.approve_ix(&owner.pubkey(), &account);
    assert!(send(&mut env.svm, &[full], &owner, &[&owner]));
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 10_000);
}

#[test]
fn regression_h02_the_guardian_cant_pause_indefinitely() {
    let mut env = Env::new();
    env.create_custom(|p| {
        p.params.activate_bps = 0;
        p.params.deactivate_bps = 0;
    });
    let guardian = env.guardian.insecure_clone();
    let (owner, account) = env.registered_landlord(0);

    assert!(env.pause(&guardian));
    // Re-pausing before expiry is refused: no extensions.
    env.warp(MAX_PAUSE_SECONDS - 1);
    assert_err!(env.pause(&guardian), PauseCooldown);
    // The pause ends on its own, and the cooldown must pass before another.
    env.warp(1);
    env.airdrop_dividend(&account, 10);
    assert!(env.sweep(&owner.pubkey(), &account));
    env.warp(PAUSE_COOLDOWN_SECONDS - 1);
    assert_err!(env.pause(&guardian), PauseCooldown);
    env.warp(1);
    assert!(env.pause(&guardian));
}

#[test]
fn regression_h02_renouncing_removes_the_guardian() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let guardian = env.guardian.insecure_clone();
    assert!(env.renounce(&admin));
    assert_eq!(env.config_state().guardian, Pubkey::default());
    assert_err!(env.pause(&guardian), NotGuardian);
}

#[test]
fn regression_l03_sweeps_cant_be_frozen_on_by_renouncing_with_zero_thresholds() {
    let mut env = Env::new();
    env.create_active();
    let admin = env.admin();
    assert_err!(env.renounce(&admin), RenounceThresholds);
    env.change_params(|p| {
        p.activate_bps = 1_000;
        p.deactivate_bps = 499;
    });
    assert_err!(env.renounce(&admin), RenounceThresholds);
    env.change_params(|p| p.deactivate_bps = 500);
    assert!(env.renounce(&admin));
}

#[test]
fn regression_m02_a_raised_coin_transfer_fee_halts_buybacks() {
    // A fee raise scheduled for a later epoch is refused as soon as it's scheduled.
    let mut env = funded_pool_env(50_000 * UNIT);
    let mint = env.inst.coin_mint;
    env.poke(&mint, |d| d[PENIS_NEWER_FEE_BPS..PENIS_NEWER_FEE_BPS + 2].copy_from_slice(&9_900u16.to_le_bytes()));
    assert_err!(env.buy(), FeeTooHigh);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 50_000 * UNIT);

    // And so is a raised current fee.
    let mut env = funded_pool_env(50_000 * UNIT);
    let mint = env.inst.coin_mint;
    env.poke(&mint, |d| d[PENIS_OLDER_FEE_BPS..PENIS_OLDER_FEE_BPS + 2].copy_from_slice(&600u16.to_le_bytes()));
    assert_err!(env.buy(), FeeTooHigh);

    // The current 3% is within the ceiling.
    let mut env = funded_pool_env(50_000 * UNIT);
    assert!(env.buy());
}

/// The attacker's own Raydium swap of the dividend for the coin.
fn raydium_swap_ix(env: &Env, trader: &Pubkey, amount_in: u64) -> Instruction {
    let cpmm = fixtures::cpmm_program();
    let mut data = vec![143, 190, 90, 218, 196, 30, 51, 222];
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    Instruction {
        program_id: cpmm,
        accounts: vec![
            AccountMeta::new_readonly(*trader, true),
            AccountMeta::new_readonly(Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &cpmm).0, false),
            AccountMeta::new_readonly(fixtures::key(fixtures::AMM_CONFIG), false),
            AccountMeta::new(env.inst.pool, false),
            AccountMeta::new(env.inst.dividend_account(trader), false),
            AccountMeta::new(env.inst.coin_account(trader), false),
            AccountMeta::new(fixtures::key(fixtures::POOL_PUMP_VAULT), false),
            AccountMeta::new(fixtures::key(fixtures::POOL_PENIS_VAULT), false),
            AccountMeta::new_readonly(TOKEN_2022, false),
            AccountMeta::new_readonly(TOKEN_2022, false),
            AccountMeta::new_readonly(env.inst.dividend_mint, false),
            AccountMeta::new_readonly(env.inst.coin_mint, false),
            AccountMeta::new(fixtures::key(fixtures::OBSERVATION), false),
        ],
        data,
    }
}

#[test]
fn regression_m03_a_same_transaction_front_run_cant_move_the_price_floor() {
    let mut env = funded_pool_env(5_000 * UNIT + tip_on(5_000 * UNIT));
    let (attacker, pump) = env.new_landlord(0);
    env.set_balance(&pump, 20_000_000 * UNIT);
    let caller_tip_account = env.inst.dividend_account(&attacker.pubkey());
    assert_eq!(caller_tip_account, pump);

    // Push the pool ~10% of its PUMP reserve, then buy in the same transaction.
    let ixs = [raydium_swap_ix(&env, &attacker.pubkey(), 1_000_000 * UNIT), env.buyback_ix(&attacker.pubkey(), 1)];
    assert_err!(send(&mut env.svm, &ixs, &attacker, &[&attacker]), PriceAboveTwap);
    assert_eq!(token_balance(&env.svm, &env.coin_vault()), 0);

    // A small move stays within the band and doesn't lower the floor, which is
    // anchored to the TWAP.
    let ixs = [raydium_swap_ix(&env, &attacker.pubkey(), 10_000 * UNIT), env.buyback_ix(&attacker.pubkey(), 1)];
    assert!(send(&mut env.svm, &ixs, &attacker, &[&attacker]));
}

#[test]
fn regression_m04_a_third_party_sweep_cant_lower_the_baseline() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(1_000 * UNIT);
    let (cold, _) = env.new_landlord(0);

    // The landlord moves 900 of their own to another wallet for a while.
    let out = env.dividend_transfer_ix(&owner.pubkey(), &cold.pubkey(), 900 * UNIT);
    assert!(send(&mut env.svm, &[out], &owner, &[&owner]));
    // Anyone sweeps during the dip: nothing moves, and the baseline stays.
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(env.landlord_state(&owner.pubkey()).baseline, 1_000 * UNIT);
    // The landlord moves it back; the next sweep takes nothing.
    let back = env.dividend_transfer_ix(&cold.pubkey(), &owner.pubkey(), 900 * UNIT);
    assert!(send(&mut env.svm, &[back], &cold, &[&cold]));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 1_000 * UNIT);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);
}

#[test]
fn regression_m05_opting_back_in_keeps_what_arrived_while_opted_out() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    env.revoke(&owner);
    // Dividends arrive while opted out.
    env.airdrop_dividend(&account, 500 * UNIT);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotDelegated);

    // The website re-opts-in with approve + resync in one transaction.
    let ixs = [env.approve_ix(&owner.pubkey(), &account), env.resync_ix(&owner.pubkey())];
    assert!(send(&mut env.svm, &ixs, &owner, &[&owner]));
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 0);
    assert_eq!(token_balance(&env.svm, &account), 500 * UNIT);

    // Only new drops are swept.
    env.airdrop_dividend(&account, 20 * UNIT);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 20 * UNIT);
}

#[test]
fn regression_m06_sweeps_and_buybacks_fail_closed_if_the_dividend_hook_is_switched_on() {
    use spl_token_2022::extension::{transfer_hook::TransferHook, BaseStateWithExtensionsMut, StateWithExtensionsMut};
    use spl_token_2022::state::Mint as MintState;

    let mut env = pool_env_with(50_000 * UNIT, |p| {
        p.params.activate_bps = 0;
        p.params.deactivate_bps = 0;
        p.params.min_stake_bps = 0;
    });
    let (owner, account) = env.registered_landlord(0);
    env.set_balance(&account, 100 * UNIT);
    assert!(env.sweep(&owner.pubkey(), &account));
    env.set_balance(&account, 100 * UNIT);

    let pump_mint = env.inst.dividend_mint;
    let mut acc = env.svm.get_account(&pump_mint).unwrap();
    {
        let mut state = StateWithExtensionsMut::<MintState>::unpack(&mut acc.data).unwrap();
        let hook = state.get_extension_mut::<TransferHook>().unwrap();
        // Any executable program as the hook.
        hook.program_id = Some(endowment::id()).try_into().unwrap();
    }
    env.svm.set_account(pump_mint, acc).unwrap();

    assert_err!(env.sweep(&owner.pubkey(), &account), TransferHookEnabled);
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT);
    let vault = token_balance(&env.svm, &env.dividend_vault());
    assert_err!(env.buy(), TransferHookEnabled);
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), vault);
}

#[test]
fn regression_m08_an_oversized_per_transaction_cap_still_buys() {
    // 600k PUMP is several percent of the pool's PUMP side: more than 3% impact.
    let mut env = pool_env_with(10_000_000 * UNIT, |p| {
        p.params.max_buy_per_tx = 600_000 * UNIT;
        p.params.max_buy_per_day = 600_000 * UNIT;
        p.params.max_price_impact_bps = 300;
    });
    let reserve = token_balance(&env.svm, &fixtures::key(fixtures::POOL_PUMP_VAULT));
    assert!(env.buy());
    let spent = env.config_state().total_dividend_spent;
    // Sized by the pool's depth, not the cap.
    assert!(spent > 0 && spent < 600_000 * UNIT);
    assert!(spent <= reserve * 300 / 20_000);
    assert!(token_balance(&env.svm, &env.coin_vault()) > 0);
}

#[test]
fn regression_i09_dust_is_skipped_without_using_up_the_interval() {
    let mut env = pool_env_with(50 * UNIT, |p| p.params.min_buy_amount = 100 * UNIT);
    assert_err!(env.buy(), NothingToBuy);
    assert_eq!(env.config_state().last_buy_at, 0);
    let vault = env.dividend_vault();
    env.set_balance(&vault, 10_000 * UNIT);
    assert!(env.buy());
}

#[test]
fn regression_l13_coin_sent_to_the_vault_doesnt_reach_the_milestone() {
    let mut env = pool_env_with(50_000 * UNIT, |p| p.params.buy_bps = 5_000);
    let coin_vault = env.coin_vault();
    env.set_balance(&coin_vault, CONTRIBUTION_CAP + 1);
    assert!(env.buy());
    let config = env.config_state();
    // Still all buying: the milestone counts only coin the endowment bought.
    assert!(!config.milestone_reached);
    assert_eq!((config.total_lp_tokens, config.total_dividend_spent), (0, MAX_BUY_PER_TX));
}

#[test]
fn regression_i19_retiring_doesnt_change_how_buybacks_spend() {
    let mut env = pool_env_with(50_000 * UNIT, |p| p.params.buy_bps = 5_000);
    let admin = env.admin();
    assert!(env.retire(&admin));
    assert!(env.buy());
    let config = env.config_state();
    assert!(!config.milestone_reached);
    assert_eq!((config.total_lp_tokens, config.total_dividend_spent), (0, MAX_BUY_PER_TX));
}

#[test]
fn regression_l04_registering_and_counting_are_refused_while_paused() {
    let mut env = Env::new();
    env.create();
    let (owner, _) = env.registered_holder(0, 10 * UNIT);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    let (newcomer, _) = env.new_landlord(0);
    assert_err!(env.register(&newcomer), Paused);
    assert_err!(env.count(), Paused);
    // Leaving still works.
    assert!(env.deregister(&owner));
}
