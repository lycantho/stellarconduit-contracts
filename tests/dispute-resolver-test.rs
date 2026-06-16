//! # Dispute Resolver — Integration Test Suite
//!
//! Full root-level integration tests for the Dispute Resolver contract.

extern crate std;

use dispute_resolver::{errors::ContractError, types::{AdminCouncil, DisputeStatus, RelayChainProof}, DisputeResolverContract, DisputeResolverContractClient};
use soroban_sdk::{testutils::{Address as _, Ledger}, Address, BytesN, Env};

fn setup<'a>() -> (Env, DisputeResolverContractClient<'a>) {
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
    client.initialize(&council, &100u32);

    (env, client)
}

fn setup_disputants(env: &Env) -> (Address, Address, ed25519_dalek::SigningKey, ed25519_dalek::SigningKey) {
    let initiator = Address::generate(env);
    let respondent = Address::generate(env);

    let initiator_sk = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let initiator_pk_bytes: [u8; 32] = initiator_sk.verifying_key().to_bytes();
    let initiator_pk = BytesN::from_array(env, &initiator_pk_bytes);

    let respondent_sk = ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]);
    let respondent_pk_bytes: [u8; 32] = respondent_sk.verifying_key().to_bytes();
    let respondent_pk = BytesN::from_array(env, &respondent_pk_bytes);

    env.as_contract(&env.current_contract().unwrap_or_else(|| panic!()), || {
        dispute_resolver::storage::set_public_key(env, &initiator, &initiator_pk);
        dispute_resolver::storage::set_public_key(env, &respondent, &respondent_pk);
    });

    (initiator, respondent, initiator_sk, respondent_sk)
}

fn create_proof(env: &Env, sk: &ed25519_dalek::SigningKey, chain_hash_bytes: &[u8; 32], sequence: u64) -> RelayChainProof {
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

fn assert_event(env: &Env, action: &str) {
    let mut found = false;
    let events = env.events().all();
    for event in events.iter() {
        let (_contract, topics, _data) = event;
        if topics.len() == 2
            && topics.get(0).unwrap() == env.bytes_new_from_slice("dispute_resolver".as_bytes())
            && topics.get(1).unwrap() == env.bytes_new_from_slice(action.as_bytes())
        {
            found = true;
            break;
        }
    }
    assert!(found, "expected event '{}' to be emitted", action);
}

#[test]
fn test_raise_dispute_creates_entry() {
    let (env, client) = setup();
    let (initiator, _, init_sk, _) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let proof = create_proof(&env, &init_sk, &chain_hash, 10);

    let dispute_id = client.raise_dispute(&initiator, &tx_id, &proof);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.dispute_id, dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Open);
    assert_eq!(dispute.tx_id, tx_id);
    assert_eq!(dispute.initiator, initiator);
    assert_eq!(dispute.resolve_by, env.ledger().sequence() as u64 + 100);
    assert_event(&env, "raise");
}

#[test]
fn test_raise_dispute_tx_already_disputed() {
    let (env, client) = setup();
    let (initiator, _, init_sk, _) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let proof = create_proof(&env, &init_sk, &chain_hash, 10);

    client.raise_dispute(&initiator, &tx_id, &proof);
    let duplicate = client.try_raise_dispute(&initiator, &tx_id, &proof);
    assert_eq!(duplicate, Err(Ok(ContractError::DuplicateDispute)));
}

#[test]
fn test_raise_dispute_invalid_respondent() {
    let (env, client) = setup();
    let (initiator, _, init_sk, _) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let proof = create_proof(&env, &init_sk, &chain_hash, 10);

    let dispute_id = client.raise_dispute(&initiator, &tx_id, &proof);
    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.respondent, None);
    assert_eq!(dispute.initiator, initiator);
}

#[test]
fn test_respond_with_valid_proof() {
    let (env, client) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &init_proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.status, DisputeStatus::Responded);
    assert_eq!(dispute.respondent, Some(respondent.clone()));
    assert_event(&env, "respond");
}

#[test]
fn test_respond_after_deadline() {
    let (env, client) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &init_proof);

    env.ledger().with_mut(|l| l.sequence_number += 101);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    let result = client.try_respond(&respondent, &dispute_id, &resp_proof);
    assert_eq!(result, Err(Ok(ContractError::ResolutionWindowExpired)));
}

#[test]
fn test_respond_already_resolved() {
    let (env, client) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &init_proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);
    client.resolve(&dispute_id);

    let result = client.try_respond(&respondent, &dispute_id, &resp_proof);
    assert_eq!(result, Err(Ok(ContractError::NotOpen)));
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_respond_unauthorized() {
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

    let initiator = Address::generate(&env);
    let respondent = Address::generate(&env);
    let init_sk = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let resp_sk = ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]);

    let init_pk_bytes: [u8; 32] = init_sk.verifying_key().to_bytes();
    let init_pk = BytesN::from_array(&env, &init_pk_bytes);
    let resp_pk_bytes: [u8; 32] = resp_sk.verifying_key().to_bytes();
    let resp_pk = BytesN::from_array(&env, &resp_pk_bytes);

    env.as_contract(&contract_id, || {
        dispute_resolver::storage::set_public_key(&env, &initiator, &init_pk);
        dispute_resolver::storage::set_public_key(&env, &respondent, &resp_pk);
    });

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);
}

#[test]
fn test_resolve_selects_correct_winner() {
    let (env, client) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 20);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &init_proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    let ruling = client.resolve(&dispute_id);
    assert_eq!(ruling.winner, respondent);
    assert_eq!(ruling.loser, initiator);
    assert_event(&env, "resolve");
}

#[test]
fn test_resolve_before_response_window() {
    let (env, client) = setup();
    let (initiator, _, init_sk, _) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &init_proof);

    let result = client.try_resolve(&dispute_id);
    assert_eq!(result, Err(Ok(ContractError::ResolutionWindowActive)));
}

#[test]
fn test_resolve_already_resolved() {
    let (env, client) = setup();
    let (initiator, respondent, init_sk, resp_sk) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &init_proof);

    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);
    client.resolve(&dispute_id);

    let result = client.try_resolve(&dispute_id);
    assert_eq!(result, Err(Ok(ContractError::DisputeAlreadyResolved)));
}

#[test]
fn test_resolve_expired_dispute() {
    let (env, client) = setup();
    let (initiator, _, init_sk, _) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &init_proof);

    env.ledger().with_mut(|l| l.sequence_number += 101);

    let ruling = client.resolve(&dispute_id);
    assert_eq!(ruling.winner, initiator);
    assert_eq!(ruling.loser, initiator);
    assert!(ruling.reason.contains("Respondent failed to respond"));
    assert_event(&env, "resolve");
}

#[test]
fn test_get_dispute_returns_data() {
    let (env, client) = setup();
    let (initiator, _, init_sk, _) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &proof);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.dispute_id, dispute_id);
    assert_eq!(dispute.tx_id, tx_id);
    assert_eq!(dispute.status, DisputeStatus::Open);
}

#[test]
fn test_get_dispute_not_found() {
    let (_env, client) = setup();
    let result = client.try_get_dispute(&999u64);
    assert_eq!(result, Err(Ok(ContractError::DisputeNotFound)));
}

#[test]
fn test_get_ruling_after_resolve() {
    let (env, client) = setup();
    let (initiator, _, init_sk, _) = setup_disputants(&env);
    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &proof);

    env.ledger().with_mut(|l| l.sequence_number += 101);
    let resolved = client.resolve(&dispute_id);
    let ruling = client.get_ruling(&dispute_id);

    assert_eq!(resolved.dispute_id, ruling.dispute_id);
    assert_eq!(resolved.winner, ruling.winner);
    assert_eq!(resolved.reason, ruling.reason);
}

#[test]
fn test_get_ruling_before_resolve() {
    let (_env, client) = setup();
    let (initiator, _, init_sk, _) = setup_disputants(&_env);
    let tx_id = BytesN::from_array(&_env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    let proof = create_proof(&_env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &tx_id, &proof);

    let result = client.try_get_ruling(&dispute_id);
    assert_eq!(result, Err(Ok(ContractError::DisputeNotFound)));
}
