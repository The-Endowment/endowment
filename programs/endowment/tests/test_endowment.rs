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
            ACTIVE_MAX_AGE_SECS, AUTHORITY_SEED, CONFIG_SEED, COUNT_INTERVAL_SECS, COUNT_TIMEOUT_SECS,
            FLAGSHIP_COIN_MINT, LANDLORD_SEED, MAX_PAUSE_SECONDS, MIN_ATTEST_SPACING_SECS, PARAM_APPLY_GRACE_SECONDS,
            PARAM_EXPIRY_SECONDS, PARAM_TIMELOCK_SECONDS, PAUSE_COOLDOWN_SECONDS, REQUIRED_ATTESTATIONS,
        },
        error::EndowmentError,
        state::{Config, CreateParams, Landlord, Params},
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
const MAX_TWAP_DEVIATION_BPS: u16 = 500;
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
            // CU_LOG=1: print compute used per successful endowment instruction,
            // to compare builds (see scripts/test.sh).
            if std::env::var("CU_LOG").is_ok() {
                if let Some(ix) = ixs.iter().find(|ix| ix.program_id == endowment::id()) {
                    let disc: String = ix.data.iter().take(8).map(|b| format!("{b:02x}")).collect();
                    println!("CU_OK {disc} {}", meta.compute_units_consumed);
                }
            }
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
    create_mint_with(svm, program, MintSpec { authority: Some(*authority), ..Default::default() })
}

/// A coin mint as the mint policy requires it: no mint or freeze authority.
/// Tests credit coin balances directly (`Env::mint_coin`).
fn create_coin_mint(svm: &mut LiteSVM, program: &Pubkey) -> Pubkey {
    create_mint_with(svm, program, MintSpec::default())
}

/// What a test mint carries. Extensions need Token-2022.
#[derive(Default, Clone, Copy)]
struct MintSpec {
    authority: Option<Pubkey>,
    freeze: Option<Pubkey>,
    fee_bps: Option<u16>,
    hook: bool,
    permanent_delegate: bool,
}

fn create_mint_with(svm: &mut LiteSVM, program: &Pubkey, spec: MintSpec) -> Pubkey {
    use spl_token_2022::extension::{
        permanent_delegate::PermanentDelegate, transfer_fee::TransferFeeConfig, transfer_hook::TransferHook,
        BaseStateWithExtensionsMut, ExtensionType, StateWithExtensionsMut,
    };
    let mint = Keypair::new().pubkey();
    let base = Mint {
        mint_authority: spec.authority.into(),
        supply: 0,
        decimals: DECIMALS,
        is_initialized: true,
        freeze_authority: spec.freeze.into(),
    };
    let mut types = vec![];
    if spec.fee_bps.is_some() {
        types.push(ExtensionType::TransferFeeConfig);
    }
    if spec.hook {
        types.push(ExtensionType::TransferHook);
    }
    if spec.permanent_delegate {
        types.push(ExtensionType::PermanentDelegate);
    }
    let data = if types.is_empty() {
        let mut data = vec![0u8; Mint::LEN];
        Mint::pack(base, &mut data).unwrap();
        data
    } else {
        let len = ExtensionType::try_calculate_account_len::<Mint>(&types).unwrap();
        let mut data = vec![0u8; len];
        let mut state = StateWithExtensionsMut::<Mint>::unpack_uninitialized(&mut data).unwrap();
        if let Some(bps) = spec.fee_bps {
            let fee = state.init_extension::<TransferFeeConfig>(true).unwrap();
            for f in [&mut fee.older_transfer_fee, &mut fee.newer_transfer_fee] {
                f.transfer_fee_basis_points = bps.into();
                f.maximum_fee = u64::MAX.into();
            }
        }
        if spec.hook {
            let hook = state.init_extension::<TransferHook>(true).unwrap();
            hook.authority = Some(Pubkey::new_unique()).try_into().unwrap();
        }
        if spec.permanent_delegate {
            let delegate = state.init_extension::<PermanentDelegate>(true).unwrap();
            delegate.delegate = Some(Pubkey::new_unique()).try_into().unwrap();
        }
        state.base = base;
        state.pack_base();
        state.init_account_type().unwrap();
        data
    };
    svm.set_account(
        mint,
        solana_account::Account {
            lamports: svm.minimum_balance_for_rent_exemption(data.len()),
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
    // Its AMM config: a 0.25% trade fee.
    let amm_config = Pubkey::new_unique();
    let mut config = vec![0u8; 236];
    config[..8].copy_from_slice(&[218, 244, 33, 104, 203, 203, 43, 111]);
    config[12..20].copy_from_slice(&2_500u64.to_le_bytes());
    svm.set_account(
        amm_config,
        solana_account::Account {
            lamports: svm.minimum_balance_for_rent_exemption(config.len()),
            data: config,
            owner: fixtures::cpmm_program(),
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    let pool = Pubkey::new_unique();
    let mut data = vec![0u8; 637];
    data[..8].copy_from_slice(&[247, 237, 227, 245, 215, 195, 222, 70]);
    data[8..40].copy_from_slice(amm_config.as_ref());
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
        max_twap_deviation_bps: MAX_TWAP_DEVIATION_BPS,
        min_buy_amount: 0,
        min_buy_interval_secs: INTERVAL,
        tip_bps: 25,
        buy_bps: 10_000,
        activate_bps: 3_000,
        deactivate_bps: 2_500,
        min_stake_bps: 10,
        refresher: refresher().pubkey(),
    }
}

/// The key every test endowment names as its refresher (the keeper, in production).
fn refresher() -> Keypair {
    Keypair::new_from_array([42u8; 32])
}

/// The integration tests run the build made with `--features test-flagship`,
/// whose flagship creator is this key (scripts/test.sh). Test-only.
fn test_flagship_creator() -> Keypair {
    const SECRET: [u8; 64] = [
        104, 177, 208, 128, 67, 223, 194, 18, 44, 248, 71, 113, 134, 22, 45, 148, 141, 6, 253, 114, 174, 94, 117,
        107, 106, 234, 140, 201, 128, 126, 151, 175, 243, 132, 106, 223, 166, 235, 79, 147, 240, 76, 5, 135, 184,
        254, 206, 54, 25, 161, 50, 196, 68, 55, 100, 178, 2, 18, 245, 101, 69, 110, 159, 198,
    ];
    Keypair::try_from(&SECRET[..]).unwrap()
}

fn flagship_config() -> Pubkey {
    config_pda(&FLAGSHIP_COIN_MINT, &test_flagship_creator().pubkey())
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
    /// Every (config, owner) registered through `register`, for counts to find.
    known: Vec<(Pubkey, Pubkey)>,
}

impl Env {
    fn base() -> (LiteSVM, Keypair, Keypair, Keypair) {
        let mut svm = LiteSVM::new();
        // Built by scripts/test.sh with `--features test-flagship`.
        let bytes = include_bytes!(concat!(env!("CARGO_TARGET_TMPDIR"), "/../deploy-test/endowment.so"));
        svm.add_program(endowment::id(), bytes).unwrap();
        let creator = Keypair::new();
        let guardian = Keypair::new();
        let mint_authority = Keypair::new();
        for kp in [&creator, &guardian, &mint_authority, &refresher()] {
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
        let coin_mint = create_coin_mint(&mut svm, &coin_program);
        let dividend_mint = create_mint(&mut svm, &mint_authority.pubkey(), &dividend_program);
        Self::with_mints(svm, creator, guardian, mint_authority, (coin_mint, coin_program), (dividend_mint, dividend_program))
    }

    /// Token-2022 mints built to `coin` and `dividend`, and a stand-in pool.
    fn with_specs(coin: MintSpec, dividend: MintSpec) -> Self {
        let (mut svm, creator, guardian, mint_authority) = Self::base();
        let coin_mint = create_mint_with(&mut svm, &TOKEN_2022, coin);
        let dividend_mint =
            create_mint_with(&mut svm, &TOKEN_2022, MintSpec { authority: Some(mint_authority.pubkey()), ..dividend });
        Self::with_mints(svm, creator, guardian, mint_authority, (coin_mint, TOKEN_2022), (dividend_mint, TOKEN_2022))
    }

    fn with_mints(
        mut svm: LiteSVM,
        creator: Keypair,
        guardian: Keypair,
        mint_authority: Keypair,
        (coin_mint, coin_program): (Pubkey, Pubkey),
        (dividend_mint, dividend_program): (Pubkey, Pubkey),
    ) -> Self {
        let pool = fake_pool(&mut svm, [coin_mint, dividend_mint], fixtures::cpmm_program());
        Env {
            svm,
            guardian,
            mint_authority,
            inst: Inst { creator, coin_mint, dividend_mint, coin_program, dividend_program, pool },
            known: vec![],
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
        let mut env = Env {
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
            known: vec![],
        };
        // The fixture's recorded prices end a day before this clock and average
        // 4.5% away from its final price, so buys would (rightly) be refused.
        // Tests start from an hour of steady trading at the current price.
        steady_history(&mut env);
        env
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
        ata(&authority_pda(&flagship_config()), &self.inst.dividend_mint, &self.inst.dividend_program)
    }

    /// Creates the flagship endowment (the test build's flagship creator, on
    /// this pool, with the defaults), which creates its dividend vault.
    fn create_flagship_vault(&mut self) {
        let mut flagship = self.inst.clone();
        flagship.creator = test_flagship_creator();
        self.svm.airdrop(&flagship.creator.pubkey(), 10_000_000_000).unwrap();
        let guardian = self.guardian.pubkey();
        assert!(self.create_inst(&flagship, params(guardian, 0)));
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
            flagship_dividend_vault: ata(&authority_pda(&flagship_config()), &inst.dividend_mint, &inst.dividend_program),
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
        self.buyback_ix_with(self.buyback_accounts(caller), min_out)
    }

    /// As the website builds it: the flagship's vault is passed writable, and
    /// the flagship's coin mint and config as the remaining accounts, only when
    /// this endowment donates.
    fn buyback_ix_with(&self, accounts: endowment::accounts::Buyback, min_out: u64) -> Instruction {
        let donates = Env::config_at(&self.svm, &accounts.config).donation_bps > 0;
        let vault = accounts.flagship_dividend_vault;
        let mut metas = accounts.to_account_metas(None);
        if donates {
            for meta in metas.iter_mut().filter(|m| m.pubkey == vault) {
                meta.is_writable = true;
            }
            metas.push(AccountMeta::new_readonly(FLAGSHIP_COIN_MINT, false));
            metas.push(AccountMeta::new_readonly(flagship_config(), false));
        }
        Instruction::new_with_bytes(endowment::id(), &endowment::instruction::Buyback { min_out }.data(), metas)
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
        let ix = self.buyback_ix_with(accounts, min_out);
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

    /// The admin applies a matured proposal (only the admin can in its first day).
    fn apply_params(&mut self) -> bool {
        let admin = self.admin();
        self.apply_params_as(&admin)
    }

    fn apply_params_as(&mut self, caller: &Keypair) -> bool {
        let ix = Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::ApplyParams {}.data(),
            endowment::accounts::ApplyParams { caller: caller.pubkey(), config: self.config() }.to_account_metas(None),
        );
        send(&mut self.svm, &[ix], caller, &[caller])
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

    /// Propose retiring, wait out the timelock, retire.
    fn retire_now(&mut self, signer: &Keypair) {
        assert!(self.retire(signer));
        self.warp(PARAM_TIMELOCK_SECONDS);
        assert!(self.retire(signer));
        assert!(self.config_state().retired);
    }

    fn renounce(&mut self, signer: &Keypair) -> bool {
        self.admin_call(signer, endowment::instruction::RenounceAdmin {}.data())
    }

    /// Owners of every landlord of `config` that still exists, in registration order.
    fn landlords_of(&self, config: &Pubkey) -> Vec<Pubkey> {
        let mut owners: Vec<Pubkey> = vec![];
        for (c, owner) in &self.known {
            if c == config && !owners.contains(owner) && exists(&self.svm, &landlord_pda(config, owner)) {
                owners.push(*owner);
            }
        }
        owners
    }

    fn landlords(&self) -> Vec<Pubkey> {
        self.landlords_of(&self.config())
    }

    fn landlord_at(svm: &LiteSVM, landlord: &Pubkey) -> Landlord {
        let account = svm.get_account(landlord).unwrap();
        Landlord::try_deserialize(&mut account.data.as_slice()).unwrap()
    }

    fn begin_ix(&self, config: Pubkey) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::BeginCount {}.data(),
            endowment::accounts::BeginCount { config, coin_mint: Env::config_at(&self.svm, &config).coin_mint }
                .to_account_metas(None),
        )
    }

    /// Counts these landlords of `config`: each as its record, coin and dividend account.
    fn count_landlords_ix(&self, config: Pubkey, owners: &[Pubkey]) -> Instruction {
        let landlords: Vec<Pubkey> = owners.iter().map(|o| landlord_pda(&config, o)).collect();
        self.count_records_ix(config, &landlords)
    }

    fn count_records_ix(&self, config: Pubkey, landlords: &[Pubkey]) -> Instruction {
        let mut metas = endowment::accounts::CountLandlords { config }.to_account_metas(None);
        for landlord in landlords {
            let state = Env::landlord_at(&self.svm, landlord);
            metas.push(AccountMeta::new(*landlord, false));
            metas.push(AccountMeta::new_readonly(state.coin_account, false));
            metas.push(AccountMeta::new_readonly(state.dividend_account, false));
        }
        Instruction::new_with_bytes(endowment::id(), &endowment::instruction::CountLandlords {}.data(), metas)
    }

    fn refresh_ix_by(&self, config: Pubkey, owners: &[Pubkey], caller: &Pubkey) -> Instruction {
        let mut metas = endowment::accounts::RefreshLandlords { config, caller: *caller }.to_account_metas(None);
        for owner in owners {
            let landlord = landlord_pda(&config, owner);
            let state = Env::landlord_at(&self.svm, &landlord);
            metas.push(AccountMeta::new(landlord, false));
            metas.push(AccountMeta::new_readonly(state.coin_account, false));
            metas.push(AccountMeta::new_readonly(state.dividend_account, false));
        }
        Instruction::new_with_bytes(endowment::id(), &endowment::instruction::RefreshLandlords {}.data(), metas)
    }

    /// The refresher reads these landlords of `config`, in one transaction.
    fn attest_ix(&self, config: Pubkey, owners: &[Pubkey]) -> Instruction {
        self.refresh_ix_by(config, owners, &refresher().pubkey())
    }

    fn attest_of(&mut self, config: Pubkey, owners: &[Pubkey]) -> bool {
        let ixs = [compute_limit_ix(1_400_000), self.attest_ix(config, owners)];
        let r = refresher();
        send(&mut self.svm, &ixs, &r, &[&r])
    }

    fn attest(&mut self, owners: &[Pubkey]) -> bool {
        self.attest_of(self.config(), owners)
    }

    /// One refresher pass over every landlord of `config`, 8 per transaction.
    fn attest_pass_of(&mut self, config: Pubkey) -> bool {
        for batch in self.landlords_of(&config).chunks(8) {
            if !self.attest_of(config, batch) {
                return false;
            }
        }
        true
    }

    /// The refresher's passes before a count, as the keeper runs them:
    /// REQUIRED_ATTESTATIONS passes, MIN_ATTEST_SPACING_SECS apart.
    fn attest_all_of(&mut self, config: Pubkey) -> bool {
        for pass in 0..REQUIRED_ATTESTATIONS {
            if pass > 0 {
                self.warp(MIN_ATTEST_SPACING_SECS);
            }
            if !self.attest_pass_of(config) {
                return false;
            }
        }
        true
    }

    /// `attest` REQUIRED_ATTESTATIONS times, spaced (the same landlords each time).
    fn attest_fully(&mut self, owners: &[Pubkey]) -> bool {
        for pass in 0..REQUIRED_ATTESTATIONS {
            if pass > 0 {
                self.warp(MIN_ATTEST_SPACING_SECS);
            }
            if !self.attest(owners) {
                return false;
            }
        }
        true
    }

    fn finish_ix(&self, config: Pubkey) -> Instruction {
        Instruction::new_with_bytes(
            endowment::id(),
            &endowment::instruction::FinishCount {}.data(),
            endowment::accounts::FinishCount { config }.to_account_metas(None),
        )
    }

    /// Sends from a fresh, funded wallet: the count's instructions are permissionless.
    fn crank(&mut self, ixs: &[Instruction]) -> bool {
        let mut all = vec![compute_limit_ix(1_400_000)];
        all.extend_from_slice(ixs);
        let caller = self.funded();
        send(&mut self.svm, &all, &caller, &[&caller])
    }

    /// The refresher's pass, then a count begins (as the keeper runs it).
    fn begin(&mut self) -> bool {
        let config = self.config();
        if !self.attest_all_of(config) {
            return false;
        }
        let ix = self.begin_ix(config);
        self.crank(&[ix])
    }

    fn count_batch(&mut self, owners: &[Pubkey]) -> bool {
        let ix = self.count_landlords_ix(self.config(), owners);
        self.crank(&[ix])
    }

    fn finish(&mut self) -> bool {
        let ix = self.finish_ix(self.config());
        self.crank(&[ix])
    }

    /// A refresh by anyone (not the refresher): decrease-only, attests nothing.
    fn refresh(&mut self, owners: &[Pubkey]) -> bool {
        let caller = self.funded();
        let ixs = [compute_limit_ix(1_400_000), self.refresh_ix_by(self.config(), owners, &caller.pubkey())];
        send(&mut self.svm, &ixs, &caller, &[&caller])
    }

    /// A whole daily count of `config` as the keeper runs it: the refresher's
    /// pass over every landlord, then begin, every landlord in batches of 8, finish.
    fn count_for(&mut self, config: Pubkey) -> bool {
        if !self.attest_all_of(config) {
            return false;
        }
        let begin = self.begin_ix(config);
        if !self.crank(&[begin]) {
            return false;
        }
        for batch in self.landlords_of(&config).chunks(8) {
            let ix = self.count_landlords_ix(config, batch);
            if !self.crank(&[ix]) {
                return false;
            }
        }
        let finish = self.finish_ix(config);
        self.crank(&[finish])
    }

    fn count(&mut self) -> bool {
        self.count_for(self.config())
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

    /// Coin mints have no mint authority: credit the balance and the supply directly.
    fn mint_coin(&mut self, owner: &Pubkey, amount: u64) {
        let (mint, account) = (self.inst.coin_mint, self.inst.coin_account(owner));
        self.poke(&account, |d| {
            let balance = u64::from_le_bytes(d[64..72].try_into().unwrap()) + amount;
            d[64..72].copy_from_slice(&balance.to_le_bytes());
        });
        self.poke(&mint, |d| {
            let supply = u64::from_le_bytes(d[36..44].try_into().unwrap()) + amount;
            d[36..44].copy_from_slice(&supply.to_le_bytes());
        });
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

    /// Approve + register in one transaction, as the website sends it.
    fn register(&mut self, owner: &Keypair) -> bool {
        let account = self.inst.dividend_account(&owner.pubkey());
        let ixs = [self.approve_ix(&owner.pubkey(), &account), self.register_ix(&owner.pubkey(), &account)];
        let ok = send(&mut self.svm, &ixs, owner, &[owner]);
        if ok {
            self.known.push((self.config(), owner.pubkey()));
        }
        ok
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
        let pool = self.svm.get_account(&self.inst.pool).unwrap().data;
        let key = |at: usize| Pubkey::new_from_array(pool[at..at + 32].try_into().unwrap());
        let dividend_first = key(168) == self.inst.dividend_mint;
        let (pool_dividend_vault, pool_coin_vault) = if dividend_first { (key(72), key(104)) } else { (key(104), key(72)) };
        endowment::accounts::Sweep {
            config,
            authority: authority_pda(&config),
            landlord: landlord_pda(&config, owner),
            dividend_mint: self.inst.dividend_mint,
            dividend_account: *account,
            dividend_vault: self.dividend_vault(),
            coin_mint: self.inst.coin_mint,
            coin_vault: self.coin_vault(),
            pool_state: self.inst.pool,
            amm_config: key(8),
            pool_dividend_vault,
            pool_coin_vault,
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
        let coin_mint = create_coin_mint(&mut self.svm, &coin_program);
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
    assert_eq!(config.version, 3);
    assert_eq!(config.creator, env.inst.creator.pubkey());
    assert_eq!(config.admin, env.inst.creator.pubkey());
    assert_eq!(config.guardian, env.guardian.pubkey());
    assert_eq!((config.coin_mint, config.dividend_mint, config.pool), (env.inst.coin_mint, env.inst.dividend_mint, env.inst.pool));
    assert_eq!(config.params, base_params());
    assert_eq!(config.pending.effective_at, 0);
    assert_eq!((config.active, config.retired, config.milestone_reached), (false, false, false));
    assert_eq!((config.contribution_cap, config.donation_bps), (CONTRIBUTION_CAP, 0));
    assert_eq!(config.buy_allowance, MAX_BUY_PER_TX);
    assert_eq!((config.landlord_count, config.count.round, config.count.open), (0, 0, false));
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
    let stranger_mint = create_coin_mint(&mut env.svm, &TOKEN_2022);

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
    assert_eq!((landlord.version, landlord.baseline, landlord.config), (3, 100, env.config()));
    assert_eq!(env.config_state().landlord_count, 1);
    assert!(!landlord.snapshot_valid);
    assert_eq!(landlord.joined_round, 0);
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
fn deregistering_closes_the_record_and_a_landlord_can_register_again_cleanly() {
    let mut env = Env::new();
    env.create_active();
    let (owner, account) = env.registered_landlord(100);
    env.airdrop_dividend(&account, 50);
    assert!(env.sweep(&owner.pubkey(), &account));

    let lamports_before = env.svm.get_balance(&owner.pubkey()).unwrap();
    assert!(env.deregister(&owner));
    assert!(!exists(&env.svm, &landlord_pda(&env.config(), &owner.pubkey())));
    assert_eq!(env.config_state().landlord_count, 0);
    assert!(env.svm.get_balance(&owner.pubkey()).unwrap() > lamports_before);

    // Coming back starts fresh: the new baseline is whatever the landlord holds now.
    env.airdrop_dividend(&account, 400);
    assert!(env.register(&owner));
    let landlord = env.landlord_state(&owner.pubkey());
    assert_eq!((landlord.baseline, landlord.total_contributed), (500, 0));
    assert_eq!(env.config_state().landlord_count, 1);
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

    // Once the time is up, only the admin can apply it for a day, then anyone.
    env.warp(1);
    assert_err!(env.apply_params_as(&stranger), ApplyGrace);
    env.warp(PARAM_APPLY_GRACE_SECONDS);
    assert!(env.apply_params_as(&stranger));
    let config = env.config_state();
    assert_eq!(config.params, next);
    assert_eq!(config.pending.effective_at, 0);
    // The allowance never exceeds the new per-transaction cap.
    assert_eq!(config.buy_allowance, UNIT);
    assert_err!(env.apply_params(), NoPendingParams);
}

#[test]
fn regression_r2roles04_a_matured_proposal_is_the_admins_to_apply_for_a_day_and_then_expires() {
    let mut env = Env::new();
    env.create();
    let admin = env.admin();
    let stranger = env.funded();
    let mut next = base_params();
    next.tip_bps = 10;
    assert!(env.propose(&admin, next));
    env.warp(PARAM_TIMELOCK_SECONDS);
    // Nobody can race the admin's cancel-and-renounce with an apply.
    assert_err!(env.apply_params_as(&stranger), ApplyGrace);
    assert!(env.cancel_params(&admin));
    // A proposal nobody applies expires.
    assert!(env.propose(&admin, next));
    env.warp(PARAM_TIMELOCK_SECONDS + PARAM_EXPIRY_SECONDS + 1);
    assert_err!(env.apply_params(), ProposalExpired);
    assert_err!(env.apply_params_as(&stranger), ProposalExpired);
    // Renouncing needs nothing pending.
    env.change_params(|p| {
        p.activate_bps = 3_000;
        p.deactivate_bps = 2_500;
    });
    assert!(env.propose(&admin, next));
    assert_err!(env.renounce(&admin), PendingChange);
    assert!(env.cancel_params(&admin));
    assert!(env.renounce(&admin));
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
    assert_err!(env.renounce(&admin), PendingChange);
    assert!(env.cancel_params(&admin));
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
fn regression_r2roles05_the_liquidity_share_buys_while_the_pool_has_deposits_disabled() {
    let mut env = milestone_env();
    let pool = env.inst.pool;
    env.poke(&pool, |d| d[POOL_STATUS] |= 1);
    let before = env.config_state();
    assert!(env.buy());
    let config = env.config_state();
    // No liquidity, and nothing waits: the whole buy was spent buying.
    assert_eq!(config.total_lp_tokens, 0);
    assert_eq!(config.total_liquidity_dividend, 0);
    assert_eq!(config.total_dividend_spent - before.total_dividend_spent, MAX_BUY_PER_TX);

    env.poke(&pool, |d| d[POOL_STATUS] &= !1);
    env.warp(DAY);
    assert!(env.buy());
    assert!(env.config_state().total_lp_tokens > 0);
}

#[test]
fn sweeps_keep_flowing_after_the_milestone() {
    let mut env = pool_env_with(10_000 * UNIT, |p| {
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
    // Proposed, announced, and only effective after the timelock (R2-ROLES-02).
    assert!(env.retire(&admin));
    let config = env.config_state();
    assert_eq!((config.retired, config.retire_at), (false, env.now() + PARAM_TIMELOCK_SECONDS));
    env.warp(PARAM_TIMELOCK_SECONDS - 1);
    assert_err!(env.retire(&admin), RetireNotReady);
    env.airdrop_dividend(&account, 10);
    assert!(env.sweep(&owner.pubkey(), &account));
    // It can be withdrawn in between.
    assert!(env.cancel_params(&admin));
    assert_eq!(env.config_state().retire_at, 0);
    env.retire_now(&admin);
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
    env.retire_now(&admin);
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

    // B's count (or refresh) can't read or write A's landlord in place of its own.
    let landlord_a = landlord_pda(&a.config(), &owner.pubkey());
    let begin_b = env.begin_ix(b.config());
    assert!(env.crank(&[begin_b]));
    let foreign = env.count_records_ix(b.config(), &[landlord_a]);
    assert_err!(env.crank(&[foreign]), InvalidCountAccount);
    let caller = env.funded();
    let mut foreign_refresh = env.refresh_ix_by(a.config(), &[owner.pubkey()], &caller.pubkey());
    foreign_refresh.accounts[0] = AccountMeta::new(b.config(), false);
    assert_err!(send(&mut env.svm, &[foreign_refresh], &caller, &[&caller]), InvalidCountAccount);

    // A's landlord can't be deregistered through B.
    assert!(!env.deregister_with(&owner, b.config(), landlord_a));
    assert_eq!(Env::config_at(&env.svm, &a.config()).landlord_count, 1);
    assert!(env.deregister(&owner));
    assert_eq!(Env::config_at(&env.svm, &a.config()).landlord_count, 0);
    assert_eq!(Env::config_at(&env.svm, &b.config()).landlord_count, 1);
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
    for owner in &owners {
        assert!(env.landlord_state(&owner.pubkey()).snapshot_valid);
    }

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
    let config = env.config();
    assert!(env.attest_pass_of(config));
    let begin = env.begin_ix(config);
    assert_err!(env.crank(&[begin.clone()]), CountTooSoon);
    let started = env.config_state().count.started_at;
    env.warp(started + COUNT_INTERVAL_SECS - 1 - env.now());
    assert_err!(env.crank(&[begin.clone()]), CountTooSoon);
    env.warp(1);
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    assert_err!(env.crank(&[begin]), Paused);
    env.warp(MAX_PAUSE_SECONDS);
    assert!(env.count());
}

#[test]
fn count_batches_take_each_landlord_once_with_its_own_accounts() {
    let (mut env, owners) = counted_env();
    let config = env.config();
    let o: Vec<Pubkey> = owners.iter().map(|k| k.pubkey()).collect();

    // Nothing to count before a round begins.
    let early = env.count_landlords_ix(config, &o[..1]);
    assert_err!(env.crank(&[early]), NoOpenCount);
    assert_err!(env.finish(), NoOpenCount);
    assert!(env.begin());
    assert_err!(env.begin(), CountOpen);

    // A forged coin account in a landlord's place.
    let stranger = env.new_landlord(0).0;
    let mut forged = env.count_landlords_ix(config, &o[..1]);
    forged.accounts[2] = AccountMeta::new_readonly(env.inst.coin_account(&stranger.pubkey()), false);
    assert_err!(env.crank(&[forged]), InvalidCountAccount);
    // Coin and dividend accounts swapped.
    let mut swapped = env.count_landlords_ix(config, &o[..1]);
    let (coin, dividend) = (swapped.accounts[2].pubkey, swapped.accounts[3].pubkey);
    swapped.accounts[2] = AccountMeta::new_readonly(dividend, false);
    swapped.accounts[3] = AccountMeta::new_readonly(coin, false);
    assert_err!(env.crank(&[swapped]), InvalidCountAccount);
    // A landlord record passed read-only.
    let mut readonly = env.count_landlords_ix(config, &o[..1]);
    readonly.accounts[1] = AccountMeta::new_readonly(readonly.accounts[1].pubkey, false);
    assert_err!(env.crank(&[readonly]), InvalidCountAccount);
    // The same landlord twice in one batch.
    let twice = env.count_landlords_ix(config, &[o[0], o[0]]);
    assert_err!(env.crank(&[twice]), NotInCount);

    // Batches of any size and order; a landlord can't be counted again this round.
    assert!(env.count_batch(&[o[2], o[0]]));
    assert_err!(env.count_batch(&o[..1]), NotInCount);
    // Not every landlord counted yet, and not timed out.
    assert_err!(env.finish(), CountIncomplete);
    assert!(env.count_batch(&o[1..2]));
    assert!(env.finish());
    let config_state = env.config_state();
    assert_eq!((config_state.count.round, config_state.count.open), (1, false));
    assert_eq!((config_state.count.expected, config_state.count.counted), (3, 3));
}

#[test]
fn a_count_left_open_can_be_finished_by_anyone_after_its_timeout() {
    let (mut env, owners) = counted_env();
    // Round 1 records balances; round 2 is started and only partly counted.
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.begin());
    assert!(env.count_batch(&[owners[1].pubkey()]));
    assert_err!(env.finish(), CountIncomplete);
    env.warp(COUNT_TIMEOUT_SECS - 1);
    assert_err!(env.finish(), CountIncomplete);
    env.warp(1);
    assert!(env.finish());
    // Only the counted landlord (15%) counts; the others count zero this round.
    let config = env.config_state();
    assert_eq!((config.last_count_bps, config.count.counted, config.count.expected), (1_500, 1, 3));
    // The next day's count starts normally and reads everyone.
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 3_000);
}

#[test]
fn landlords_joining_or_leaving_mid_count_dont_break_it() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.begin());
    assert!(env.count_batch(&[owners[0].pubkey()]));

    // A newcomer joins mid-round: not part of this round.
    let (late, _) = env.registered_holder(0, 50_000 * UNIT);
    assert_err!(env.count_batch(&[late.pubkey()]), NotInCount);
    // A counted landlord leaves: its 10% comes back out of the tally.
    assert!(env.deregister(&owners[0]));
    // An uncounted landlord leaves: the round no longer waits for it.
    assert!(env.deregister(&owners[2]));
    let count = env.config_state().count;
    assert_eq!((count.expected, count.counted, count.committed), (1, 0, 0));
    assert!(env.count_batch(&[owners[1].pubkey()]));
    assert!(env.finish());
    // Only owners[1]: 150,000 of 1,050,000.
    assert_eq!(env.config_state().last_committed, 150_000 * UNIT);
    // The newcomer is counted from the next round on (zero at its first read).
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.config_state().last_committed, 200_000 * UNIT);
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
// Landlords: unlimited, and prunable by anyone once they no longer qualify.
// ---------------------------------------------------------------------------

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
    assert_eq!(env.config_state().landlord_count, 2);

    // Below the minimum stake: pruned.
    env.burn_coin(&owners[2], 49_500 * UNIT);
    assert!(env.prune(&owners[2].pubkey()));
    assert_eq!(env.config_state().landlord_count, 1);
    assert!(env.count());
}

#[test]
fn there_is_no_limit_on_landlords() {
    let mut env = Env::new();
    env.create();
    const LANDLORDS: u64 = 64;
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 1_000_000 * UNIT - LANDLORDS * 5_000 * UNIT);
    for _ in 0..LANDLORDS {
        env.registered_holder(0, 5_000 * UNIT);
    }
    assert_eq!(env.config_state().landlord_count, LANDLORDS as u32);

    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    // Count the second round by hand to measure a full batch of 8.
    assert!(env.begin());
    let owners = env.landlords();
    let mut max_cu = 0;
    for batch in owners.chunks(8) {
        assert!(env.count_batch(batch));
        max_cu = max_cu.max(last_cu());
    }
    assert!(env.finish());
    // 64 × 5,000 = 320,000 of 1,000,000: 32%, on.
    let config = env.config_state();
    assert_eq!((config.last_count_bps, config.active, config.count.counted), (3_200, true, 64));
    println!("count_landlords, batch of 8: {max_cu} CU");
    assert!(max_cu < 400_000);
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
fn regression_h01_moving_coin_to_another_landlord_mid_count_doesnt_count_it_twice() {
    let (mut env, s) = sybil_env();
    let (a, b) = (s[0].pubkey(), s[1].pubkey());
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);

    // Count a (it held 10% since the last count), move the 10% to b, count b.
    assert!(env.begin());
    assert!(env.count_batch(&[a]));
    let ix = env.coin_transfer_ix(&a, &b, 100_000 * UNIT);
    assert!(send(&mut env.svm, &[ix], &s[0], &[&s[0]]));
    assert!(env.count_batch(&[b, s[2].pubkey()]));
    assert!(env.finish());
    // b is credited only what it held at its previous count (its 2,000).
    // 100,000 + 2,000 + 2,000 of 1,000,000.
    assert_eq!(env.committed_bps(), 1_040);
}

#[test]
fn regression_h01_moving_coin_inside_one_transaction_mid_count_doesnt_count_it_twice() {
    let (mut env, s) = sybil_env();
    let (a, b) = (s[0].pubkey(), s[1].pubkey());
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.begin());
    // count a, move the 10% to b, count b, all in one transaction.
    let ixs = [
        compute_limit_ix(1_400_000),
        env.count_landlords_ix(env.config(), &[a]),
        env.coin_transfer_ix(&a, &b, 100_000 * UNIT),
        env.count_landlords_ix(env.config(), &[b, s[2].pubkey()]),
    ];
    assert!(send(&mut env.svm, &ixs, &s[0], &[&s[0]]));
    assert!(env.finish());
    assert_eq!(env.committed_bps(), 1_040);
}

/// The residual the refresh closes: coin cycled between two landlord wallets so
/// each holds it whenever it is counted.
fn cycle_one_round(env: &mut Env, s: &[Keypair]) {
    let (a, b) = (s[0].pubkey(), s[1].pubkey());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.begin());
    assert!(env.count_batch(&[a, s[2].pubkey()]));
    let to_b = env.coin_transfer_ix(&a, &b, 100_000 * UNIT);
    assert!(send(&mut env.svm, &[to_b], &s[0], &[&s[0]]));
    assert!(env.count_batch(&[b]));
    assert!(env.finish());
    let back = env.coin_transfer_ix(&b, &a, 100_000 * UNIT);
    assert!(send(&mut env.svm, &[back], &s[1], &[&s[1]]));
}

#[test]
fn regression_r2cnt01_coin_cycled_around_every_count_counts_once() {
    let (mut env, s) = sybil_env();
    assert!(env.count());
    cycle_one_round(&mut env, &s); // b records 102,000 at its count read...
    cycle_one_round(&mut env, &s); // ...but the refresher's pass found 2,000 there.
    // Only real holdings: 100,000 + 2,000 + 2,000 (was 2,040 bps before the fix).
    assert_eq!(env.committed_bps(), 1_040);
}

#[test]
fn regression_h01_a_refresh_between_counts_stops_coin_cycled_between_landlords() {
    let (mut env, s) = sybil_env();
    let (a, b) = (s[0].pubkey(), s[1].pubkey());
    assert!(env.count());
    cycle_one_round(&mut env, &s);
    // Between counts, anyone refreshes every landlord in one transaction: the
    // coin can only be in one of the two wallets at that moment.
    let everyone = env.landlords();
    assert!(env.refresh(&everyone));
    assert_eq!(env.landlord_state(&b).snapshot, 2_000 * UNIT);
    assert_eq!(env.landlord_state(&a).snapshot, 100_000 * UNIT);
    cycle_one_round(&mut env, &s);
    // Only real holdings count: 100,000 + 2,000 + 2,000.
    assert_eq!(env.committed_bps(), 1_040);
}

#[test]
fn a_refresh_only_ever_lowers_and_is_open_to_anyone() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    let o = owners[0].pubkey();
    // Coin added after a count isn't credited early by a refresh.
    env.mint_coin(&o, 50_000 * UNIT);
    assert!(env.refresh(&[o]));
    assert_eq!(env.landlord_state(&o).snapshot, 100_000 * UNIT);
    // A drop is recorded at once.
    env.burn_coin(&owners[0], 120_000 * UNIT);
    assert!(env.refresh(&[o]));
    assert_eq!(env.landlord_state(&o).snapshot, 30_000 * UNIT);
    // A refresh with a foreign coin account is refused.
    let caller = env.funded();
    let mut forged = env.refresh_ix_by(env.config(), &[o], &caller.pubkey());
    forged.accounts[3] = AccountMeta::new_readonly(env.inst.coin_account(&owners[1].pubkey()), false);
    assert_err!(send(&mut env.svm, &[forged], &caller, &[&caller]), InvalidCountAccount);
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

    // Re-delegating fully restores it, from the second count on: the first
    // re-records the balance.
    let full = env.approve_ix(&owner.pubkey(), &account);
    assert!(send(&mut env.svm, &[full], &owner, &[&owner]));
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.committed_bps(), 0);
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
    env.retire_now(&admin);
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

// ---------------------------------------------------------------------------
// Regressions: the round-2 audit's exploits (audit-pocs/round2), which must now fail.
// ---------------------------------------------------------------------------

/// R2-CNT-01's chain: K registered wallets, X (10% of supply) really held once
/// plus 2,000 dust (the minimum stake) in each of the others.
fn chain_env(k: usize, x: u64, dust: u64) -> (Env, Vec<Keypair>) {
    let mut env = Env::new();
    env.create();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 1_000_000 * UNIT - x - (k as u64 - 1) * dust);
    let mut w = vec![];
    for i in 0..k {
        let (s, _) = env.registered_holder(0, if i == 0 { x } else { dust });
        w.push(s);
    }
    (env, w)
}

/// One round of the PoC's chain in a single transaction: begin, then count W0,
/// move X to W1, count W1, ... count Wk-1, then finish; afterwards X goes back to W0.
fn chain_round(env: &mut Env, w: &[Keypair], x: u64) {
    env.warp(COUNT_INTERVAL_SECS);
    let config = env.config();
    let mut ixs = vec![compute_limit_ix(1_400_000), env.begin_ix(config)];
    for i in 0..w.len() {
        ixs.push(env.count_landlords_ix(config, &[w[i].pubkey()]));
        if i + 1 < w.len() {
            ixs.push(env.coin_transfer_ix(&w[i].pubkey(), &w[i + 1].pubkey(), x));
        }
    }
    ixs.push(env.finish_ix(config));
    let signers: Vec<&Keypair> = w[..w.len() - 1].iter().collect();
    assert!(send(&mut env.svm, &ixs, &w[0], &signers), "chain round failed");
    let last = &w[w.len() - 1];
    let back = env.coin_transfer_ix(&last.pubkey(), &w[0].pubkey(), x);
    assert!(send(&mut env.svm, &[back], last, &[last]));
}

#[test]
fn regression_r2cnt01_a_chain_of_k_wallets_counts_one_holding_once() {
    const K: usize = 4;
    let (x, dust) = (100_000 * UNIT, 2_000 * UNIT);
    let (mut env, w) = chain_env(K, x, dust);
    let owners: Vec<Pubkey> = w.iter().map(|k| k.pubkey()).collect();
    assert!(env.count()); // first read: records balances
    for _ in 0..3 {
        // The refresher's passes (one transaction each) land at times the
        // attacker doesn't choose; the coin is in one wallet then.
        assert!(env.attest_fully(&owners));
        chain_round(&mut env, &w, x);
        let config = env.config_state();
        // Before the fix: 4 × 10% + dust = 4,060 bps, active. Now: what is really held.
        assert_eq!(config.last_committed, x + (K as u64 - 1) * dust);
        assert_eq!(config.last_count_bps, 1_060);
        assert!(!config.active);
    }
}

#[test]
fn regression_r2cnt01_without_the_refreshers_attestation_nobody_counts() {
    let (mut env, w) = chain_env(4, 100_000 * UNIT, 2_000 * UNIT);
    assert!(env.count());
    // Rounds run by the attacker alone (no refresher pass) can't even begin
    // (R3-RF-03), so nothing counts.
    env.warp(COUNT_INTERVAL_SECS);
    let begin = env.begin_ix(env.config());
    assert_err!(env.crank(&[begin.clone()]), NotAttested);
    // An anyone-refresh doesn't attest either.
    let owners: Vec<Pubkey> = w.iter().map(|k| k.pubkey()).collect();
    assert!(env.refresh(&owners));
    assert_err!(env.crank(&[begin]), NotAttested);
    assert_eq!(env.config_state().last_committed, 0);
    // An endowment with no refresher at all never activates (fails safe).
    let mut env = Env::new();
    env.create();
    env.change_params(|p| p.refresher = Pubkey::default());
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 500_000 * UNIT);
    let (whale, _) = env.registered_holder(0, 500_000 * UNIT);
    assert!(env.count());
    for _ in 0..3 {
        env.warp(COUNT_INTERVAL_SECS);
        assert_err!(env.count(), NotAttested);
    }
    assert_eq!(env.config_state().last_committed, 0);
    assert_eq!(env.landlord_state(&whale.pubkey()).attestations, 0);
}

#[test]
fn r2cnt01_the_bound_one_holding_counts_twice_only_if_it_wins_the_race_at_every_read() {
    // The quantified bound. A landlord counts only after REQUIRED_ATTESTATIONS
    // spaced reads by the refresher. To have X counted in two wallets, the
    // attacker must move it between the two wallets' reads in every one of
    // those passes (R3-RF-02): winning once is not enough.
    let (x, dust) = (100_000 * UNIT, 2_000 * UNIT);
    let (mut env, w) = chain_env(3, x, dust);
    let (a, b, c) = (w[0].pubkey(), w[1].pubkey(), w[2].pubkey());
    // A pass split in two with a hop in between: a is read holding X, then b is.
    let split_pass = |env: &mut Env| {
        assert!(env.attest(&[a]));
        let hop = env.coin_transfer_ix(&a, &b, x);
        assert!(send(&mut env.svm, &[hop], &w[0], &[&w[0]]));
        assert!(env.attest(&[b, c]));
        let back = env.coin_transfer_ix(&b, &a, x);
        assert!(send(&mut env.svm, &[back], &w[1], &[&w[1]]));
    };
    assert!(env.count());
    // One-transaction passes, then the chain: X once.
    assert!(env.attest_fully(&[a, b, c]));
    chain_round(&mut env, &w, x);
    assert_eq!(env.config_state().last_committed, x + 2 * dust);
    // One pass split (the attacker won one race), two whole: still X once.
    split_pass(&mut env);
    env.warp(MIN_ATTEST_SPACING_SECS);
    assert!(env.attest(&[a, b, c]));
    env.warp(MIN_ATTEST_SPACING_SECS);
    assert!(env.attest(&[a, b, c]));
    chain_round(&mut env, &w, x);
    assert_eq!(env.config_state().last_committed, x + 2 * dust);
    // Only when every pass is split, and the attacker wins every race: 2 × X.
    for pass in 0..REQUIRED_ATTESTATIONS {
        if pass > 0 {
            env.warp(MIN_ATTEST_SPACING_SECS);
        }
        split_pass(&mut env);
    }
    chain_round(&mut env, &w, x);
    assert_eq!(env.config_state().last_committed, 2 * x + 2 * dust);
}

#[test]
fn regression_r2cnt02_approving_counting_and_revoking_in_one_transaction_counts_nothing() {
    let mut env = Env::new();
    env.create();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 600_000 * UNIT);
    let (whale, whale_div) = env.registered_holder(0, 400_000 * UNIT);
    // The whale stays revoked except inside its own count transaction.
    env.revoke(&whale);
    let daily = |env: &mut Env| {
        // The refresher's pass finds it revoked: its record is dropped.
        assert!(env.attest_all_of(env.config()));
        let config = env.config();
        let revoke =
            spl_token_2022::instruction::revoke(&env.inst.dividend_program, &whale_div, &whale.pubkey(), &[]).unwrap();
        let ixs = [
            compute_limit_ix(1_400_000),
            env.begin_ix(config),
            env.approve_ix(&whale.pubkey(), &whale_div),
            env.count_landlords_ix(config, &[whale.pubkey()]),
            revoke,
            env.finish_ix(config),
        ];
        assert!(send(&mut env.svm, &ixs, &whale, &[&whale]));
    };
    daily(&mut env);
    for _ in 0..3 {
        env.warp(COUNT_INTERVAL_SECS);
        daily(&mut env);
        // Before the fix: 4,000 bps and active.
        let config = env.config_state();
        assert_eq!((config.last_count_bps, config.active), (0, false));
    }
}

#[test]
fn regression_r2cnt05_a_landlord_closed_after_the_batch_was_built_is_skipped() {
    let (mut env, owners) = counted_env();
    let config = env.config();
    assert!(env.begin());
    let o: Vec<Pubkey> = owners.iter().map(|k| k.pubkey()).collect();
    let batch = env.count_landlords_ix(config, &o);
    let refresh = env.refresh_ix_by(config, &o, &refresher().pubkey());
    assert!(env.deregister(&owners[1]));
    // Neither the count nor the refresh batch fails for the rest.
    assert!(env.crank(&[batch]));
    let r = refresher();
    assert!(send(&mut env.svm, &[refresh], &r, &[&r]));
    assert!(env.finish());
    assert_eq!(env.config_state().count.counted, 2);
}

#[test]
fn regression_r2iso04_a_reassigned_coin_account_counts_zero_and_can_be_pruned() {
    let mut env = Env::with_programs(TOKEN, TOKEN);
    env.create();
    let outsider = env.new_landlord(0).0;
    env.mint_coin(&outsider.pubkey(), 600_000 * UNIT);
    let (a, _) = env.registered_holder(0, 300_000 * UNIT);
    let (b, _) = env.registered_holder(0, 100_000 * UNIT);
    assert!(env.count());
    // Original SPL Token lets an owner hand its token account to someone else.
    let coin = env.inst.coin_account(&b.pubkey());
    let new_owner = Pubkey::new_unique();
    let ix = spl_token_2022::instruction::set_authority(
        &TOKEN,
        &coin,
        Some(&new_owner),
        spl_token_2022::instruction::AuthorityType::AccountOwner,
        &b.pubkey(),
        &[],
    )
    .unwrap();
    assert!(send(&mut env.svm, &[ix], &b, &[&b]));
    // Counting doesn't fail: b simply holds nothing for the endowment.
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert_eq!(env.config_state().last_committed, 300_000 * UNIT);
    // And anyone can remove it.
    assert!(env.prune(&b.pubkey()));
    let _ = a;
}

#[test]
fn regression_r2roles10_a_mid_round_parameter_change_doesnt_change_the_rounds_minimum_stake() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    // owners[2] drops just under 5%; a proposal raising the minimum stake to 5%
    // matures while a round is open.
    env.burn_coin(&owners[2], UNIT);
    let admin = env.admin();
    let mut next = env.config_state().params;
    next.min_stake_bps = 500;
    assert!(env.propose(&admin, next));
    env.warp(PARAM_TIMELOCK_SECONDS - 60);
    assert!(env.begin());
    assert!(env.count_batch(&[owners[0].pubkey()]));
    env.warp(60);
    assert!(env.apply_params());
    assert!(env.count_batch(&[owners[1].pubkey(), owners[2].pubkey()]));
    assert!(env.finish());
    // It still counts in this round: the round began at a 10 bps minimum.
    assert_eq!(env.config_state().last_committed, 299_999 * UNIT);
}

#[test]
fn regression_r2cnt08_a_pause_doesnt_run_out_an_open_rounds_clock() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.begin());
    assert!(env.count_batch(&[owners[0].pubkey()]));
    let guardian = env.guardian.insecure_clone();
    assert!(env.pause(&guardian));
    env.warp(MAX_PAUSE_SECONDS);
    // The pause is over, but the round's timeout starts again from its end.
    assert_err!(env.finish(), CountIncomplete);
    assert!(env.count_batch(&[owners[1].pubkey(), owners[2].pubkey()]));
    assert!(env.finish());
    assert_eq!(env.committed_bps(), 3_000);
}

#[test]
fn regression_i13_sweeps_stop_when_no_count_has_finished_for_three_days() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(env.config_state().active);
    let owner = owners[0].insecure_clone();
    let account = env.inst.dividend_account(&owner.pubkey());
    env.airdrop_dividend(&account, 10);
    env.warp(ACTIVE_MAX_AGE_SECS);
    assert!(env.sweep(&owner.pubkey(), &account));
    env.airdrop_dividend(&account, 10);
    env.warp(1);
    assert_err!(env.sweep(&owner.pubkey(), &account), CountStale);
    // The next count switches them back on.
    assert!(env.count());
    assert!(env.sweep(&owner.pubkey(), &account));
}

/// A landlord of an endowment created with `f`, with 100 in dividend above its baseline.
fn sweep_env(coin: MintSpec, dividend: MintSpec) -> (Env, Keypair, Pubkey) {
    let mut env = Env::with_specs(coin, dividend);
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, 100);
    (env, owner, account)
}

#[test]
fn regression_r2t01_sweeps_fail_closed_when_the_coins_hook_is_switched_on() {
    use spl_token_2022::extension::{transfer_hook::TransferHook, BaseStateWithExtensionsMut, StateWithExtensionsMut};
    let (mut env, owner, account) = sweep_env(MintSpec { hook: true, ..Default::default() }, MintSpec::default());
    // A hook authority with no program passes the mint policy, and sweeps run.
    assert!(env.sweep(&owner.pubkey(), &account));
    env.airdrop_dividend(&account, 100);
    let mint = env.inst.coin_mint;
    let mut acc = env.svm.get_account(&mint).unwrap();
    {
        let mut state = StateWithExtensionsMut::<Mint>::unpack(&mut acc.data).unwrap();
        state.get_extension_mut::<TransferHook>().unwrap().program_id = Some(endowment::id()).try_into().unwrap();
    }
    env.svm.set_account(mint, acc).unwrap();
    // Buybacks can't trade a hooked coin, so the dividend stays with the landlord.
    assert_err!(env.sweep(&owner.pubkey(), &account), TransferHookEnabled);
    assert_eq!(token_balance(&env.svm, &account), 100);
}

#[test]
fn regression_r2t02_sweeps_fail_closed_on_a_dividend_fee_above_the_cap() {
    let (mut env, owner, account) = sweep_env(MintSpec::default(), MintSpec { fee_bps: Some(100), ..Default::default() });
    // 1% is within the 5% cap.
    assert!(env.sweep(&owner.pubkey(), &account));
    env.airdrop_dividend(&account, 100);
    // A fee authority raises it (scheduled or current): the sweep refuses.
    let mint = env.inst.dividend_mint;
    env.poke(&mint, |d| {
        use spl_token_2022::extension::{transfer_fee::TransferFeeConfig, BaseStateWithExtensionsMut, StateWithExtensionsMut};
        let mut state = StateWithExtensionsMut::<Mint>::unpack(&mut d[..]).unwrap();
        state.get_extension_mut::<TransferFeeConfig>().unwrap().newer_transfer_fee.transfer_fee_basis_points =
            9_000u16.into();
    });
    assert_err!(env.sweep(&owner.pubkey(), &account), FeeTooHigh);
    assert_eq!(token_balance(&env.svm, &account), 100);
}

#[test]
fn regression_r2roles01_sweeps_fail_closed_when_buybacks_cant_run() {
    // The pool's swaps disabled.
    let mut env = funded_pool_env(0);
    env.change_params(|p| {
        p.activate_bps = 0;
        p.deactivate_bps = 0;
        p.min_stake_bps = 0;
    });
    let (owner, account) = env.registered_landlord(0);
    env.set_balance(&account, 100 * UNIT);
    let pool = env.inst.pool;
    env.poke(&pool, |d| d[POOL_STATUS] |= 4);
    assert_err!(env.sweep(&owner.pubkey(), &account), PoolSwapDisabled);
    env.poke(&pool, |d| d[POOL_STATUS] &= !4);
    // The pool's fee above the cap.
    env.poke(&fixtures::key(fixtures::AMM_CONFIG), |d| {
        d[AMM_TRADE_FEE_RATE..AMM_TRADE_FEE_RATE + 8].copy_from_slice(&25_000u64.to_le_bytes())
    });
    assert_err!(env.sweep(&owner.pubkey(), &account), FeeTooHigh);
    env.poke(&fixtures::key(fixtures::AMM_CONFIG), |d| {
        d[AMM_TRADE_FEE_RATE..AMM_TRADE_FEE_RATE + 8].copy_from_slice(&2_500u64.to_le_bytes())
    });
    // The coin's transfer fee raised above the cap.
    let mint = env.inst.coin_mint;
    env.poke(&mint, |d| d[PENIS_NEWER_FEE_BPS..PENIS_NEWER_FEE_BPS + 2].copy_from_slice(&9_900u16.to_le_bytes()));
    assert_err!(env.sweep(&owner.pubkey(), &account), FeeTooHigh);
    env.poke(&mint, |d| d[PENIS_NEWER_FEE_BPS..PENIS_NEWER_FEE_BPS + 2].copy_from_slice(&300u16.to_le_bytes()));
    // A frozen vault (the pool's, here).
    let pool_vault = fixtures::key(fixtures::POOL_PUMP_VAULT);
    env.poke(&pool_vault, |d| d[108] = 2);
    assert_err!(env.sweep(&owner.pubkey(), &account), VaultFrozen);
    env.poke(&pool_vault, |d| d[108] = 1);
    assert_eq!(token_balance(&env.svm, &account), 100 * UNIT);
    // All clear: it sweeps.
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), 0);
    // A sweep can't be pointed at another pool's accounts.
    env.set_balance(&account, 100 * UNIT);
    let mut wrong = env.sweep_accounts(&owner.pubkey(), &account);
    std::mem::swap(&mut wrong.pool_dividend_vault, &mut wrong.pool_coin_vault);
    assert_err!(env.sweep_with(wrong), WrongPool);
}

#[test]
fn regression_r2t08_a_reassigned_dividend_account_isnt_swept() {
    let mut env = Env::with_programs(TOKEN, TOKEN);
    env.create_active();
    let (owner, account) = env.registered_landlord(0);
    env.airdrop_dividend(&account, 100);
    let new_owner = Pubkey::new_unique();
    let ix = spl_token_2022::instruction::set_authority(
        &TOKEN,
        &account,
        Some(&new_owner),
        spl_token_2022::instruction::AuthorityType::AccountOwner,
        &owner.pubkey(),
        &[],
    )
    .unwrap();
    assert!(send(&mut env.svm, &[ix], &owner, &[&owner]));
    assert_err!(env.sweep(&owner.pubkey(), &account), NotDelegated);
    let resync = env.resync_ix(&owner.pubkey());
    assert_err!(send(&mut env.svm, &[resync], &owner, &[&owner]), NotDelegated);
}

#[test]
fn regression_r2iso02_the_mint_policy_refuses_coins_someone_could_mint_freeze_or_take_back() {
    let owner = Some(Pubkey::new_unique());
    let cases = [
        (MintSpec { authority: owner, ..Default::default() }, MintSpec::default(), false),
        (MintSpec { freeze: owner, ..Default::default() }, MintSpec::default(), false),
        (MintSpec { permanent_delegate: true, ..Default::default() }, MintSpec::default(), false),
        (MintSpec::default(), MintSpec { permanent_delegate: true, ..Default::default() }, false),
        // A dividend may have a freeze authority and a fee within the cap; a coin a fee.
        (MintSpec { fee_bps: Some(300), ..Default::default() }, MintSpec { freeze: owner, fee_bps: Some(100), ..Default::default() }, true),
        (MintSpec { hook: true, ..Default::default() }, MintSpec { hook: true, ..Default::default() }, true),
    ];
    for (coin, dividend, ok) in cases {
        let mut env = Env::with_specs(coin, dividend);
        let p = params(env.guardian.pubkey(), 0);
        if ok {
            assert!(env.create_with(p));
        } else {
            assert_err!(env.create_with(p), UnsafeMint);
        }
    }
    // The flagship's real mints ($PENIS and PUMP) pass: every pool test creates with them.
    let mut env = Env::with_pool();
    env.create();
}

#[test]
fn regression_r2iso01_the_flagship_is_derived_and_never_donates_to_itself() {
    let mut env = Env::with_pool();
    env.inst.creator = test_flagship_creator();
    env.svm.airdrop(&env.inst.creator.pubkey(), 10_000_000_000).unwrap();
    assert_eq!(env.config(), flagship_config());
    let guardian = env.guardian.pubkey();
    assert_err!(env.create_with(params(guardian, 10)), InvalidDonation);
    assert!(env.create_with(params(guardian, 0)));
    // A donating endowment's donation lands in exactly this flagship's vault.
    let mut donor = env.inst.clone();
    donor.creator = env.funded();
    assert!(env.create_inst(&donor, params(guardian, 20)));
    env.inst = donor;
    let vault = env.dividend_vault();
    env.set_balance(&vault, 50_000 * UNIT);
    let authority = env.authority();
    env.create_lp_vault(&authority);
    assert_eq!(env.flagship_vault(), ata(&authority_pda(&flagship_config()), &env.inst.dividend_mint, &TOKEN_2022));
    // The flagship's vault exists (created with the flagship): the donation arrives.
    assert!(env.buy());
    assert_eq!(token_balance(&env.svm, &env.flagship_vault()), MAX_BUY_PER_TX * 20 / 10_000);
}

#[test]
fn regression_r2iso09_a_donation_is_skipped_until_the_flagship_vault_exists_and_needs_it_writable() {
    let mut env = pool_env_with(50_000 * UNIT, |p| p.donation_bps = 20);
    // No flagship vault yet: the buy runs and donates nothing.
    assert!(env.buy());
    assert_eq!(env.config_state().total_donated, 0);
    env.create_flagship_vault();
    env.warp(DAY);
    // Passing the vault read-only can't skip the donation.
    let caller = env.cranker();
    let accounts = env.buyback_accounts(&caller.pubkey());
    let ix = Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::Buyback { min_out: 1 }.data(),
        accounts.to_account_metas(None),
    );
    assert_err!(send(&mut env.svm, &[ix], &caller, &[&caller]), WrongFlagshipVault);
    let accounts = env.buyback_accounts(&caller.pubkey());
    assert!(env.buyback_with(&caller, accounts, 1));
    let donated = env.config_state().total_donated;
    assert!(donated > 0);
    // A frozen flagship vault doesn't stop the donor's buybacks either (R2-ISO-06).
    let flagship_vault = env.flagship_vault();
    env.poke(&flagship_vault, |d| d[108] = 2);
    env.warp(DAY);
    assert!(env.buy());
    assert_eq!(env.config_state().total_donated, donated);
}

#[test]
fn regression_r2t10_a_missing_lp_vault_routes_the_liquidity_share_to_buying() {
    let mut env = Env::with_pool();
    env.create_custom(|p| {
        p.contribution_cap = 1;
        p.params.buy_bps = 5_000;
    });
    let vault = env.dividend_vault();
    env.set_balance(&vault, 100_000 * UNIT);
    assert!(env.buy());
    assert!(env.config_state().milestone_reached);
    env.warp(DAY);
    // No LP vault was ever created: the buy runs, all of it buying.
    let before = env.config_state().total_dividend_spent;
    assert!(env.buy());
    let config = env.config_state();
    assert_eq!((config.total_lp_tokens, config.total_dividend_spent - before), (0, MAX_BUY_PER_TX));
}

/// Replaces the pool's price history with an hour of records every 60 s at the
/// current price, the last one a minute ago. Returns that price (coin per
/// dividend, Q32.32).
fn steady_history(env: &mut Env) -> u128 {
    let now = env.now() as u64;
    let pump = token_balance(&env.svm, &fixtures::key(fixtures::POOL_PUMP_VAULT)) as u128;
    let penis = token_balance(&env.svm, &fixtures::key(fixtures::POOL_PENIS_VAULT)) as u128;
    // Token 0 is PUMP, token 1 is $PENIS.
    let (p0, p1) = ((penis << 32) / pump, (pump << 32) / penis);
    env.poke(&fixtures::key(fixtures::OBSERVATION), |d| {
        d[OBSERVATIONS..OBSERVATIONS + 40 * 100].fill(0);
        for i in 0..60u64 {
            let at = OBSERVATIONS + 40 * i as usize;
            d[at..at + 8].copy_from_slice(&(now - 60 * (60 - i)).to_le_bytes());
            d[at + 8..at + 24].copy_from_slice(&(p0 * 60 * i as u128).to_le_bytes());
            d[at + 24..at + 40].copy_from_slice(&(p1 * 60 * i as u128).to_le_bytes());
        }
        d[9..11].copy_from_slice(&59u16.to_le_bytes());
        d[OBSERVATION_LAST_UPDATE..OBSERVATION_LAST_UPDATE + 8].copy_from_slice(&(now - 60).to_le_bytes());
    });
    p0
}

/// Sends dividend straight into the pool's dividend vault (no swap): the coin's
/// price in the dividend rises, and nothing is recorded in the price history.
fn donate_to_pool(env: &mut Env, bps: u64) {
    let vault = fixtures::key(fixtures::POOL_PUMP_VAULT);
    let balance = token_balance(&env.svm, &vault);
    env.set_balance(&vault, balance + balance * bps / 10_000);
}

#[test]
fn regression_r2cc01_a_transfer_into_a_quiet_pools_vault_doesnt_move_the_twap() {
    let mut env = funded_pool_env(50_000 * UNIT);
    // Buys work at the steady price...
    assert!(env.buy());
    // ...then a quiet pool: nothing traded for a day.
    env.warp(DAY);
    donate_to_pool(&mut env, 1_000);
    // Before the fix the day since the last swap was weighted at this spot price
    // and the buy went through at a 10% worse price. Now the TWAP ignores it.
    let bought = token_balance(&env.svm, &env.coin_vault());
    assert_err!(env.buy(), PriceAboveTwap);
    assert_eq!(token_balance(&env.svm, &env.coin_vault()), bought);
}

#[test]
fn regression_r2cc01_a_transfer_then_a_swap_colours_at_most_a_quarter_of_the_twap() {
    let mut env = funded_pool_env(50_000 * UNIT);
    let (trader, pump) = env.new_landlord(0);
    env.set_balance(&pump, 1_000_000 * UNIT);
    env.warp(DAY);
    // The transfer, then a swap: Raydium prices the whole quiet day at the moved price.
    donate_to_pool(&mut env, 1_000);
    let ix = raydium_swap_ix(&env, &trader.pubkey(), UNIT);
    assert!(send(&mut env.svm, &[ix], &trader, &[&trader]));
    env.warp(15);
    let ix = raydium_swap_ix(&env, &trader.pubkey(), UNIT);
    assert!(send(&mut env.svm, &[ix], &trader, &[&trader]));
    // The day counts for at most a quarter of the TWAP, so the average moves at most
    // a quarter as far as the spot price, and the spot band refuses the buy.
    assert_err!(env.buy(), PriceAboveTwap);
    assert_eq!(env.config_state().total_dividend_spent, 0);
}

#[test]
fn regression_f01_a_coalesced_base_observation_doesnt_lower_the_floor() {
    let mut env = funded_pool_env(5_000 * UNIT + tip_on(5_000 * UNIT));
    // (Already steady; this returns the price.)
    let price = steady_history(&mut env);
    let (trader, pump) = env.new_landlord(0);
    env.set_balance(&pump, 1_000_000 * UNIT);
    // The PoC: a tiny swap opens a record, another 14 s later coalesces into it,
    // a third 1 s after that opens the next record; then the window passes.
    for gap in [0, 14, 1] {
        env.warp(gap);
        let ix = raydium_swap_ix(&env, &trader.pubkey(), UNIT);
        assert!(send(&mut env.svm, &[ix], &trader, &[&trader]));
    }
    env.warp(1_790);
    let data = env.svm.get_account(&fixtures::key(fixtures::OBSERVATION)).unwrap().data;
    let twap = endowment::raydium::twap_price_x32(&data, &env.inst.pool, 0, env.now() as u64).unwrap().price_x32;
    // The price held steady throughout. Round 1 read 14 s / span low here
    // (−2.3% over its 10-minute window); now within ±7 s / span.
    let bps = twap * 10_000 / price;
    println!("twap / price = {bps} bps");
    assert!((9_960..=10_040).contains(&bps), "{bps}");
    assert!(env.buy());
}

// ---------------------------------------------------------------------------
// Regressions: the round-3 audit's exploits (audit-pocs/round3), which must now fail.
// ---------------------------------------------------------------------------

fn resign_refresher(env: &mut Env, signer: &Keypair) -> bool {
    let ix = Instruction::new_with_bytes(
        endowment::id(),
        &endowment::instruction::ResignRefresher {}.data(),
        endowment::accounts::ResignRefresher { refresher: signer.pubkey(), config: env.config() }
            .to_account_metas(None),
    );
    send(&mut env.svm, &[ix], signer, &[signer])
}

#[test]
fn regression_r3rf01_the_refresher_can_resign_after_renounce_and_then_sweeps_stop() {
    let (mut env, owners) = counted_env();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(env.config_state().active);
    let admin = env.admin();
    assert!(env.renounce(&admin));
    // Nobody else can resign it.
    let stranger = env.funded();
    assert_err!(resign_refresher(&mut env, &stranger), NotRefresher);
    // A refresher whose key may have leaked shuts itself off, with no admin left.
    assert!(resign_refresher(&mut env, &refresher()));
    assert_eq!(env.config_state().params.refresher, Pubkey::default());
    assert_err!(resign_refresher(&mut env, &refresher()), NotRefresher);
    // Sweeps stop at once, and its reads attest nothing now, so no count can start.
    assert!(!env.config_state().active);
    let owner = owners[0].insecure_clone();
    let account = env.inst.dividend_account(&owner.pubkey());
    env.airdrop_dividend(&account, 10);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotActive);
    env.warp(COUNT_INTERVAL_SECS);
    assert_err!(env.count(), NotAttested);
}

#[test]
fn regression_r3rf01_a_colluding_refresher_must_time_every_one_of_several_spaced_reads() {
    // The PoC: the refresher reads each wallet once, timed to when the holding
    // sits there. One read is no longer enough to count: each wallet is left
    // pending, and counts zero when the round times out.
    const K: usize = 4;
    let (x, dust) = (100_000 * UNIT, 2_000 * UNIT);
    let (mut env, w) = chain_env(K, x, dust);
    let owners: Vec<Pubkey> = w.iter().map(|k| k.pubkey()).collect();
    assert!(env.count());
    assert!(env.attest_fully(&owners));
    chain_round(&mut env, &w, x);
    assert_eq!(env.config_state().last_count_bps, 1_060);
    env.warp(MIN_ATTEST_SPACING_SECS);
    for i in 0..K {
        assert!(env.attest(&[owners[i]]));
        if i + 1 < K {
            let hop = env.coin_transfer_ix(&owners[i], &owners[i + 1], x);
            assert!(send(&mut env.svm, &[hop], &w[i], &[&w[i]]));
        }
    }
    let back = env.coin_transfer_ix(&owners[K - 1], &owners[0], x);
    assert!(send(&mut env.svm, &[back], &w[K - 1], &[&w[K - 1]]));
    env.warp(COUNT_INTERVAL_SECS);
    let begin = env.begin_ix(env.config());
    assert!(env.crank(&[begin]));
    assert!(env.count_batch(&owners));
    assert_eq!(env.config_state().count.counted, 0, "every wallet is pending");
    env.warp(COUNT_TIMEOUT_SECS);
    assert!(env.finish());
    let c = env.config_state();
    assert_eq!((c.last_committed, c.active), (0, false));
    // Reads less than MIN_ATTEST_SPACING_SECS apart add up to one.
    let reads = env.landlord_state(&owners[0]).attestations;
    assert!(env.attest(&owners[..1]));
    assert!(env.attest(&owners[..1]));
    assert_eq!(env.landlord_state(&owners[0]).attestations, reads + 1);
    env.warp(MIN_ATTEST_SPACING_SECS);
    assert!(env.attest(&owners[..1]));
    assert_eq!(env.landlord_state(&owners[0]).attestations, reads + 2);
}

#[test]
fn regression_r3rf03_nobody_can_zero_a_round_by_counting_before_the_refresher() {
    let (mut env, owners) = counted_env();
    let o: Vec<Pubkey> = owners.iter().map(|k| k.pubkey()).collect();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(env.config_state().active);
    // A round can't begin until the refresher has read someone since the last one began.
    env.warp(COUNT_INTERVAL_SECS);
    let begin = env.begin_ix(env.config());
    assert_err!(env.crank(&[begin.clone()]), NotAttested);
    // After one pass, a third party begins at once and counts everyone.
    assert!(env.attest(&o));
    assert!(env.crank(&[begin]));
    assert!(env.count_batch(&o));
    // Nobody was consumed as zero: they're pending, and the round can't finish.
    assert_eq!(env.config_state().count.counted, 0);
    assert_err!(env.finish(), CountIncomplete);
    // The refresher completes its reads; the count then credits what's held.
    env.warp(MIN_ATTEST_SPACING_SECS);
    assert!(env.attest(&o));
    env.warp(MIN_ATTEST_SPACING_SECS);
    assert!(env.attest(&o));
    assert!(env.count_batch(&o));
    assert!(env.finish());
    let c = env.config_state();
    assert_eq!((c.last_count_bps, c.active), (3_000, true));
}

#[test]
fn regression_r3rf04_deactivate_zero_is_refused_with_an_activation_line() {
    let (mut env, _) = counted_env();
    let admin = env.admin();
    let mut p = env.config_state().params;
    p.deactivate_bps = 0;
    assert_err!(env.propose(&admin, p), InvalidActivation);
    let guardian = env.guardian.pubkey();
    let mut create = params(guardian, 0);
    create.params.deactivate_bps = 0;
    let mut other = Env::new();
    assert_err!(other.create_with(create), InvalidActivation);
}

#[test]
fn regression_r3rf06_renounce_needs_a_refresher() {
    let (mut env, _) = counted_env();
    env.change_params(|p| p.refresher = Pubkey::default());
    let admin = env.admin();
    assert_err!(env.renounce(&admin), NoRefresher);
    env.change_params(|p| p.refresher = refresher().pubkey());
    assert!(env.renounce(&admin));
}

/// Replaces the pool's price history with a full ring of records 15 s apart
/// (as a busy pool, or a flood of dust swaps, leaves it), wrapped so the
/// latest sits mid-ring, at the current price.
fn busy_history(env: &mut Env) {
    let now = env.now() as u64;
    let pump = token_balance(&env.svm, &fixtures::key(fixtures::POOL_PUMP_VAULT)) as u128;
    let penis = token_balance(&env.svm, &fixtures::key(fixtures::POOL_PENIS_VAULT)) as u128;
    let (p0, p1) = ((penis << 32) / pump, (pump << 32) / penis);
    const LATEST: usize = 37;
    env.poke(&fixtures::key(fixtures::OBSERVATION), |d| {
        for i in 0..100u64 {
            let slot = (LATEST + 1 + i as usize) % 100;
            let at = OBSERVATIONS + 40 * slot;
            d[at..at + 8].copy_from_slice(&(now - 15 * (100 - i)).to_le_bytes());
            d[at + 8..at + 24].copy_from_slice(&(p0 * 15 * i as u128).to_le_bytes());
            d[at + 24..at + 40].copy_from_slice(&(p1 * 15 * i as u128).to_le_bytes());
        }
        d[9..11].copy_from_slice(&(LATEST as u16).to_le_bytes());
        d[OBSERVATION_LAST_UPDATE..OBSERVATION_LAST_UPDATE + 8].copy_from_slice(&(now - 15).to_le_bytes());
    });
}

#[test]
fn regression_r3tw01_a_ring_of_records_15_seconds_apart_still_buys() {
    let mut env = funded_pool_env(50_000 * UNIT);
    busy_history(&mut env);
    // 1,485 s of history, short of the full window: before the fix, TwapUnavailable.
    assert!(env.buy());
}

#[test]
fn regression_r3tw02_the_band_is_a_parameter_and_refuses_only_a_pricier_coin() {
    let mut env = funded_pool_env(50_000 * UNIT);
    // Spot 4% from the TWAP, coin pricier: within the default ±5%, it buys.
    donate_to_pool(&mut env, 400);
    assert!(env.buy());
    // Tightened to ±3% (timelocked), the same deviation is refused, as "above".
    env.change_params(|p| p.max_twap_deviation_bps = 300);
    steady_history(&mut env);
    donate_to_pool(&mut env, 400);
    assert_err!(env.buy(), PriceAboveTwap);
    // The coin 4% cheaper than its TWAP: it buys. The band is one-sided; the
    // floor, anchored to the TWAP, bounds the fill either way.
    steady_history(&mut env);
    let vault = fixtures::key(fixtures::POOL_PENIS_VAULT);
    let balance = token_balance(&env.svm, &vault);
    env.set_balance(&vault, balance + balance * 400 / 10_000);
    assert!(env.buy());
    // Out of bounds either way.
    let admin = env.admin();
    let mut p = env.config_state().params;
    p.max_twap_deviation_bps = 99;
    assert_err!(env.propose(&admin, p), InvalidBuybackLimits);
    p.max_twap_deviation_bps = 1_001;
    assert_err!(env.propose(&admin, p), InvalidBuybackLimits);
}

#[test]
fn regression_r3tw03_a_flat_price_buys_at_the_smallest_impact_and_band() {
    let mut env = funded_pool_env(50_000 * UNIT);
    env.change_params(|p| {
        p.max_price_impact_bps = 10;
        p.max_twap_deviation_bps = 100;
    });
    steady_history(&mut env);
    // Before the fix the floor sat above the pool's own quote at any impact
    // budget up to ~50 bps, and every buy reverted.
    assert!(env.buy());
}

#[test]
fn regression_r3tw04_a_transfer_then_a_swap_colours_at_most_a_quarter_of_the_twap() {
    let mut env = funded_pool_env(50_000 * UNIT);
    let before = steady_history(&mut env);
    let (trader, pump) = env.new_landlord(0);
    env.set_balance(&pump, 1_000_000 * UNIT);
    env.warp(DAY);
    // Coin 20% pricier in dividend terms, then a swap prices the whole quiet day there.
    donate_to_pool(&mut env, 2_500);
    let ix = raydium_swap_ix(&env, &trader.pubkey(), UNIT);
    assert!(send(&mut env.svm, &[ix], &trader, &[&trader]));
    env.warp(15);
    let ix = raydium_swap_ix(&env, &trader.pubkey(), UNIT);
    assert!(send(&mut env.svm, &[ix], &trader, &[&trader]));
    let data = env.svm.get_account(&fixtures::key(fixtures::OBSERVATION)).unwrap().data;
    let twap = endowment::raydium::twap_price_x32(&data, &env.inst.pool, 0, env.now() as u64).unwrap().price_x32;
    // The day weighs a quarter: the TWAP moved about a quarter of the way.
    let moved = 10_000 - twap * 10_000 / before;
    assert!((400..=600).contains(&moved), "{moved}");
    assert_err!(env.buy(), PriceAboveTwap);
}

#[test]
fn regression_r3mint01_a_sweep_never_fills_the_vault_beyond_three_days_of_buys() {
    let mut env = funded_pool_env(0);
    env.change_params(|p| {
        p.activate_bps = 0;
        p.deactivate_bps = 0;
        p.min_stake_bps = 0;
    });
    let (owner, account) = env.registered_landlord(0);
    // 50 days of the daily buy cap waiting in the landlord's account.
    let big = 50 * MAX_BUY_PER_DAY;
    env.set_balance(&account, big);
    assert!(env.sweep(&owner.pubkey(), &account));
    let cap = 3 * MAX_BUY_PER_DAY;
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), cap);
    assert_eq!(token_balance(&env.svm, &account), big - cap, "the rest stays with the landlord");
    // A full vault: a sweep is a no-op, not an error.
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &account), big - cap);
    // A buy makes room again.
    assert!(env.buy());
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), cap);
    // If PUMP's hook authority now switched a hook on, at most `cap` is stranded.
    let pump = fixtures::key(fixtures::PUMP_MINT);
    env.poke(&pump, |d| d[202..234].copy_from_slice(Pubkey::new_unique().as_ref()));
    env.warp(DAY);
    assert_err!(env.buy(), TransferHookEnabled);
    assert!(token_balance(&env.svm, &env.dividend_vault()) <= cap);
}

#[test]
fn regression_r3mint04_a_stale_older_fee_stops_halting_once_the_newer_fee_is_in_force() {
    let mut env = funded_pool_env(50_000 * UNIT);
    let epoch = env.svm.get_sysvar::<Clock>().epoch;
    let penis = fixtures::key(fixtures::PENIS_MINT);
    // The PoC: older = 90% (past), newer = 3% in force since this epoch. Now it buys.
    env.poke(&penis, |d| {
        d[PENIS_OLDER_FEE_BPS..PENIS_OLDER_FEE_BPS + 2].copy_from_slice(&9_000u16.to_le_bytes());
        d[PENIS_NEWER_FEE_BPS - 16..PENIS_NEWER_FEE_BPS - 8].copy_from_slice(&epoch.to_le_bytes());
        d[PENIS_NEWER_FEE_BPS..PENIS_NEWER_FEE_BPS + 2].copy_from_slice(&300u16.to_le_bytes());
    });
    assert!(env.buy());
    // While the older fee is still the one in force, it halts.
    env.poke(&penis, |d| {
        d[PENIS_NEWER_FEE_BPS - 16..PENIS_NEWER_FEE_BPS - 8].copy_from_slice(&(epoch + 1).to_le_bytes());
    });
    env.warp(DAY);
    assert_err!(env.buy(), FeeTooHigh);
}

#[test]
fn regression_r3mint05_creation_refuses_a_fee_already_above_the_cap() {
    let mut env = Env::with_pool();
    let penis = fixtures::key(fixtures::PENIS_MINT);
    env.poke(&penis, |d| {
        d[PENIS_OLDER_FEE_BPS..PENIS_OLDER_FEE_BPS + 2].copy_from_slice(&600u16.to_le_bytes());
        d[PENIS_NEWER_FEE_BPS..PENIS_NEWER_FEE_BPS + 2].copy_from_slice(&600u16.to_le_bytes());
    });
    let guardian = env.guardian.pubkey();
    assert_err!(env.create_with(params(guardian, 0)), FeeTooHigh);
}

#[test]
fn regression_r3mint03_a_donating_buy_must_name_the_flagship_coin() {
    let mut env = pool_env_with(50_000 * UNIT, |p| p.donation_bps = 20);
    env.create_flagship_vault();
    let caller = env.cranker();
    let accounts = env.buyback_accounts(&caller.pubkey());
    let mut ix = env.buyback_ix_with(accounts, 1);
    let config = ix.accounts.pop().unwrap();
    assert_eq!(config.pubkey, flagship_config());
    let mint = ix.accounts.pop().unwrap();
    assert_eq!(mint.pubkey, FLAGSHIP_COIN_MINT);
    // Without the flagship's coin mint, or with another mint in its place: refused.
    assert_err!(send(&mut env.svm, &[ix.clone()], &caller, &[&caller]), WrongFlagshipVault);
    ix.accounts.push(AccountMeta::new_readonly(env.inst.dividend_mint, false));
    ix.accounts.push(config.clone());
    assert_err!(send(&mut env.svm, &[ix.clone()], &caller, &[&caller]), WrongFlagshipVault);
    ix.accounts.truncate(ix.accounts.len() - 2);
    // The coin, but without the flagship's config (its vault cap), or another
    // endowment's config in its place: refused.
    ix.accounts.push(mint);
    assert_err!(send(&mut env.svm, &[ix.clone()], &caller, &[&caller]), WrongFlagshipVault);
    ix.accounts.push(AccountMeta::new_readonly(env.config(), false));
    assert_err!(send(&mut env.svm, &[ix.clone()], &caller, &[&caller]), WrongFlagshipVault);
    ix.accounts.pop();
    ix.accounts.push(config);
    assert!(send(&mut env.svm, &[ix], &caller, &[&caller]));
    assert!(env.config_state().total_donated > 0);
}

// ---------------------------------------------------------------------------
// Regressions: the final check's findings (audit-pocs/final-check), which must now fail.
// ---------------------------------------------------------------------------

#[test]
fn regression_fcr302_a_pending_proposal_cant_reinstate_a_resigned_refresher() {
    let (mut env, _owners) = counted_env();
    let admin = env.admin();
    // The probe: a routine change, proposed while the refresher is still trusted.
    let mut p = env.config_state().params;
    p.tip_bps = p.tip_bps.saturating_sub(1);
    assert!(env.propose(&admin, p));
    // The refresher suspects its key leaked and resigns: the proposal loses it too.
    assert!(resign_refresher(&mut env, &refresher()));
    assert_eq!(env.config_state().pending.params.refresher, Pubkey::default());
    // Past the timelock and the admin's grace day, a stranger applies it.
    env.warp(PARAM_TIMELOCK_SECONDS + PARAM_APPLY_GRACE_SECONDS);
    let stranger = env.funded();
    assert!(env.apply_params_as(&stranger));
    let c = env.config_state();
    assert_eq!(c.params.refresher, Pubkey::default(), "the resigned key stays out");
    assert_eq!(c.params.tip_bps, p.tip_bps, "the rest of the change applies");
    assert!(!c.active);
}

#[test]
fn regression_fcr303_resigning_switches_sweeps_off_and_voids_its_reads() {
    let (mut env, owners) = counted_env();
    let o: Vec<Pubkey> = owners.iter().map(|k| k.pubkey()).collect();
    assert!(env.count());
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.count());
    assert!(env.config_state().active);
    // A round opens on a full set of reads; the refresher resigns partway through.
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.attest_fully(&o));
    let begin = env.begin_ix(env.config());
    assert!(env.crank(&[begin]));
    assert!(env.count_batch(&o[..1]));
    assert!(resign_refresher(&mut env, &refresher()));
    let c = env.config_state();
    assert!(!c.active && !c.count.open);
    // Sweeps are off at once, not when the last count goes stale.
    let owner = owners[0].insecure_clone();
    let account = env.inst.dividend_account(&owner.pubkey());
    env.airdrop_dividend(&account, 10);
    assert_err!(env.sweep(&owner.pubkey(), &account), NotActive);
    // The round those reads were for closed without a result.
    assert_err!(env.finish(), NoOpenCount);
    // Reappointed through the timelock (the same key), it starts from nothing:
    // sweeps stay off until a new count, and its earlier reads don't carry over.
    env.change_params(|p| p.refresher = refresher().pubkey());
    assert!(!env.config_state().active);
    env.warp(COUNT_INTERVAL_SECS);
    assert!(env.attest(&o));
    for owner in &o[1..] {
        assert_eq!(env.landlord_state(owner).attestations, 1, "one fresh read, not four");
    }
    let begin = env.begin_ix(env.config());
    assert!(env.crank(&[begin]));
    assert!(env.count_batch(&o));
    assert_eq!(env.config_state().count.counted, 0, "every landlord is pending");
}

#[test]
fn regression_fcmint01_a_donation_stops_at_the_flagship_vault_cap() {
    let mut env = pool_env_with(50_000 * UNIT, |p| p.donation_bps = 30);
    env.create_flagship_vault();
    let flagship_vault = env.flagship_vault();
    // The flagship runs the defaults: three days of MAX_BUY_PER_DAY.
    let cap = 3 * MAX_BUY_PER_DAY;
    // The probe: already ten times the cap. Nothing more arrives; the share
    // stays in the donor's vault for its own buys.
    env.set_balance(&flagship_vault, 10 * cap);
    assert!(env.buy());
    assert_eq!(token_balance(&env.svm, &flagship_vault), 10 * cap);
    assert_eq!(env.config_state().total_donated, 0);
    // Just under the cap: the donation tops it up exactly.
    env.set_balance(&flagship_vault, cap - 100);
    env.warp(DAY);
    assert!(env.buy());
    assert_eq!(token_balance(&env.svm, &flagship_vault), cap);
    assert_eq!(env.config_state().total_donated, 100);
}

#[test]
fn regression_fcr304_the_vault_cap_counts_what_the_buy_interval_lets_it_spend() {
    let mut env = funded_pool_env(0);
    env.change_params(|p| {
        p.activate_bps = 0;
        p.deactivate_bps = 0;
        p.min_stake_bps = 0;
        p.min_buy_interval_secs = DAY;
    });
    // One buy of at most MAX_BUY_PER_TX a day: the cap is three of those,
    // not three of max_buy_per_day.
    assert_eq!(env.config_state().vault_cap(), 3 * MAX_BUY_PER_TX);
    let (owner, account) = env.registered_landlord(0);
    env.set_balance(&account, 50 * MAX_BUY_PER_DAY);
    assert!(env.sweep(&owner.pubkey(), &account));
    assert_eq!(token_balance(&env.svm, &env.dividend_vault()), 3 * MAX_BUY_PER_TX);
}

/// The attacker's own Raydium swap the other way: the coin for the dividend.
fn raydium_sell_ix(env: &Env, trader: &Pubkey, amount_in: u64) -> Instruction {
    let mut ix = raydium_swap_ix(env, trader, amount_in);
    ix.accounts.swap(4, 5);
    ix.accounts.swap(6, 7);
    ix.accounts.swap(10, 11);
    ix
}

#[test]
fn regression_band_an_attacker_who_pushes_the_price_down_first_only_loses() {
    // Control: the same buy with nobody in front of it.
    let mut control = funded_pool_env(5_000 * UNIT + tip_on(5_000 * UNIT));
    assert!(control.buy());
    let fair = token_balance(&control.svm, &control.coin_vault());

    let mut env = funded_pool_env(5_000 * UNIT + tip_on(5_000 * UNIT));
    let (attacker, pump) = env.new_landlord(0);
    let dump = token_balance(&env.svm, &fixtures::key(fixtures::POOL_PENIS_VAULT)) / 20;
    env.mint_coin(&attacker.pubkey(), dump);
    // Dump 5% of the pool's coin, then the buyback in the same transaction: the
    // coin sits well below its TWAP, and the buy goes through at that price.
    let ixs = [raydium_sell_ix(&env, &attacker.pubkey(), dump), env.buyback_ix(&attacker.pubkey(), 1)];
    assert!(send(&mut env.svm, &ixs, &attacker, &[&attacker]));
    let bought = token_balance(&env.svm, &env.coin_vault());
    assert!(bought > fair, "the endowment gets more coin, not less: {bought} vs {fair}");
    // The attacker buys back with everything the dump raised (and the tip),
    // and ends with less coin than it dumped.
    let raised = token_balance(&env.svm, &pump);
    let ix = raydium_swap_ix(&env, &attacker.pubkey(), raised);
    assert!(send(&mut env.svm, &[ix], &attacker, &[&attacker]));
    let back = token_balance(&env.svm, &env.inst.coin_account(&attacker.pubkey()));
    println!("attacker: dumped {dump}, got back {back}");
    assert!(back < dump);
}
