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
}

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &endowment::id()).0
}

fn send(svm: &mut LiteSVM, ixs: &[Instruction], payer: &Keypair, signers: &[&Keypair]) -> bool {
    let msg = Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &svm.latest_blockhash());
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    let ok = svm.send_transaction(tx).is_ok();
    svm.expire_blockhash();
    ok
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
        }
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
