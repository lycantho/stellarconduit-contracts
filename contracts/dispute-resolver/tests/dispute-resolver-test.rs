#![cfg(test)]

use dispute_resolver::{
    storage,
    types::{AdminCouncil, RelayChainProof},
    DisputeResolverContract, DisputeResolverContractClient,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, BytesN, Env,
};

fn setup<'a>() -> (Env, DisputeResolverContractClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(DisputeResolverContract, ());
    let client = DisputeResolverContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = AdminCouncil {
        members,
        threshold: 1,
    };
    client.initialize(&council, &100u32); // 100 ledger resolution window
    (env, client, admin)
}

fn create_proof(
    env: &Env,
    sk: &ed25519_dalek::SigningKey,
    chain_hash_bytes: &[u8; 32],
    sequence: u64,
) -> RelayChainProof {
    let chain_hash = BytesN::from_array(env, chain_hash_bytes);

    use ed25519_dalek::Signer;
    let sig = sk.sign(chain_hash_bytes.as_slice());
    let signature = BytesN::from_array(env, &sig.to_bytes());

    RelayChainProof {
        signature,
        chain_hash,
        sequence,
    }
}

fn setup_disputants(
    env: &Env,
    client: &DisputeResolverContractClient,
) -> (
    Address,
    Address,
    ed25519_dalek::SigningKey,
    ed25519_dalek::SigningKey,
) {
    let initiator = Address::generate(env);
    let respondent = Address::generate(env);

    let initiator_sk = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let initiator_pk_bytes: [u8; 32] = initiator_sk.verifying_key().to_bytes();
    let initiator_pk = BytesN::from_array(env, &initiator_pk_bytes);

    let respondent_sk = ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]);
    let respondent_pk_bytes: [u8; 32] = respondent_sk.verifying_key().to_bytes();
    let respondent_pk = BytesN::from_array(env, &respondent_pk_bytes);

    env.as_contract(&client.address, || {
        storage::set_public_key(env, &initiator, &initiator_pk);
        storage::set_public_key(env, &respondent, &respondent_pk);
    });

    (initiator, respondent, initiator_sk, respondent_sk)
}

// ── initialize() tests ────────────────────────────────────────────────────────

#[test]
fn test_initialize_success() {
    let env = Env::default();
    let contract_id = env.register(DisputeResolverContract, ());
    let client = DisputeResolverContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = AdminCouncil {
        members,
        threshold: 1,
    };

    client.initialize(&council, &100u32);

    let active_window = env.as_contract(&client.address, || storage::get_resolution_window(&env));
    assert_eq!(active_window, 100);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")] // AlreadyInitialized
fn test_initialize_already_initialized() {
    let (env, client, admin) = setup();

    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = AdminCouncil {
        members,
        threshold: 1,
    };
    client.initialize(&council, &200u32);
}

// ── raise_dispute() tests ─────────────────────────────────────────────────────

#[test]
fn test_raise_dispute_success() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);

    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);
    assert_eq!(dispute_id, 1);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, dispute_resolver::types::DisputeStatus::Open);
    assert_eq!(dispute.tx_id, tx_id);
    assert_eq!(dispute.initiator, initiator);
}

#[test]
fn test_raise_dispute_auto_increment_id() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id1 = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let init_proof1 = create_proof(&env, &init_sk, &chain_hash, 10);

    let dispute_id1 = client.raise_dispute(&initiator, &respondent, &tx_id1, &init_proof1);
    assert_eq!(dispute_id1, 1);

    let tx_id2 = BytesN::from_array(&env, &[10u8; 32]);
    let init_proof2 = create_proof(&env, &init_sk, &chain_hash, 11);

    let dispute_id2 = client.raise_dispute(&initiator, &respondent, &tx_id2, &init_proof2);
    assert_eq!(dispute_id2, 2);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // DuplicateDispute
fn test_raise_dispute_duplicate_tx_id() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);

    client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Duplicate tx_id
    client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);
}

#[test]
#[should_panic(expected = "HostError: Error(Auth, InvalidAction)")]
fn test_raise_dispute_auth_required() {
    let env = Env::default();
    // Do not call mock_all_auths()
    let contract_id = env.register(DisputeResolverContract, ());
    let client = DisputeResolverContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = AdminCouncil {
        members,
        threshold: 1,
    };
    client.initialize(&council, &100u32);

    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);

    // Will panic because require_auth() is not mocked
    client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);
}

// ── respond() tests ───────────────────────────────────────────────────────────

#[test]
fn test_respond_success() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(
        dispute.status,
        dispute_resolver::types::DisputeStatus::Responded
    );
    assert_eq!(dispute.respondent, respondent);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // NotOpen
fn test_respond_not_open() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    // Attempting to respond again when status is Responded
    client.respond(&respondent, &dispute_id, &resp_proof);
}

#[test]
#[should_panic(expected = "Error(Contract, #11)")] // ResolutionWindowExpired
fn test_respond_window_expired() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Advance ledger sequence past the resolution window (100)
    env.ledger().with_mut(|l| l.sequence_number += 101);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // DisputeNotFound
fn test_respond_dispute_not_found() {
    let (env, client, _) = setup();
    let (_, respondent, _, resp_sk) = setup_disputants(&env, &client);

    let chain_hash = [8u8; 32];
    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);

    client.respond(&respondent, &999, &resp_proof);
}

// ── resolve() tests ───────────────────────────────────────────────────────────

#[test]
fn test_resolve_initiator_wins_lower_sequence() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    // Initiator seq = 10
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Respondent seq = 15
    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    let ruling = client.resolve(&dispute_id);
    assert_eq!(ruling.winner, initiator);
    assert_eq!(ruling.loser, respondent);
}

#[test]
fn test_resolve_respondent_wins_lower_sequence() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    // Initiator seq = 20
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 20);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Respondent seq = 15
    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    let ruling = client.resolve(&dispute_id);
    assert_eq!(ruling.winner, respondent);
    assert_eq!(ruling.loser, initiator);
}

#[test]
fn test_resolve_tie_initiator_wins() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 15);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    let ruling = client.resolve(&dispute_id);
    assert_eq!(ruling.winner, initiator);
    assert_eq!(ruling.loser, respondent);
}

#[test]
fn test_resolve_no_response_expired() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Advance ledger sequence past the resolution window
    env.ledger().with_mut(|l| l.sequence_number += 101);

    let ruling = client.resolve(&dispute_id);
    assert_eq!(ruling.winner, initiator);
    assert_eq!(ruling.loser, respondent);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")] // UnauthorizedRespondent
fn test_respond_unauthorized_respondent() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);
    
    // Create a third party
    let unauthorized = Address::generate(&env);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    let unauthorized_sk = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
    let resp_proof = create_proof(&env, &unauthorized_sk, &chain_hash, 15);
    
    client.respond(&unauthorized, &dispute_id, &resp_proof);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // DisputeAlreadyResolved
fn test_resolve_already_resolved() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    client.resolve(&dispute_id);
    client.resolve(&dispute_id); // Second resolve should panic
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")] // ResolutionWindowActive
fn test_resolve_window_still_active() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Window is still active here, so it should panic as it's not Responded either
    client.resolve(&dispute_id);
}

// ── get_dispute() and get_ruling() tests ──────────────────────────────────────

#[test]
fn test_get_dispute_found() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.dispute_id, dispute_id);
    assert_eq!(dispute.status, dispute_resolver::types::DisputeStatus::Open);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // DisputeNotFound
fn test_get_dispute_not_found() {
    let (_env, client, _) = setup();
    client.get_dispute(&888);
}

#[test]
fn test_get_ruling_after_resolve() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    env.ledger().with_mut(|l| l.sequence_number += 101);

    let ruling_resolve = client.resolve(&dispute_id);
    let ruling_get = client.get_ruling(&dispute_id);

    assert_eq!(ruling_resolve.winner, ruling_get.winner);
    assert_eq!(ruling_resolve.reason, ruling_get.reason);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // DisputeNotFound
fn test_get_ruling_not_yet_resolved() {
    let (env, client, _) = setup();
    let (initiator, respondent, init_sk, _) = setup_disputants(&env, &client);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Attempt to get ruling before resolution
    client.get_ruling(&dispute_id);
}
