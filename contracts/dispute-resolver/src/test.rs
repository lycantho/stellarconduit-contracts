use super::*;
use crate::types::RelayChainProof;
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env};

fn setup_dispute<'a>(
    env: &'a Env,
) -> (
    DisputeResolverContractClient<'a>,
    Address,
    Address,
    ed25519_dalek::SigningKey,
    ed25519_dalek::SigningKey,
) {
    let contract_id = env.register(DisputeResolverContract, ());
    let client = DisputeResolverContractClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let mut members = soroban_sdk::Vec::new(env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    client.initialize(&council, &100);

    let initiator = Address::generate(env);
    let respondent = Address::generate(env);

    // Generate real Ed25519 keypairs for signing test proofs
    let initiator_sk = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let initiator_pk_bytes: [u8; 32] = initiator_sk.verifying_key().to_bytes();
    let initiator_pk = BytesN::from_array(env, &initiator_pk_bytes);

    let respondent_sk = ed25519_dalek::SigningKey::from_bytes(&[2u8; 32]);
    let respondent_pk_bytes: [u8; 32] = respondent_sk.verifying_key().to_bytes();
    let respondent_pk = BytesN::from_array(env, &respondent_pk_bytes);

    // Register public keys in storage mapping
    env.as_contract(&contract_id, || {
        storage::set_public_key(env, &initiator, &initiator_pk);
        storage::set_public_key(env, &respondent, &respondent_pk);
    });

    (client, initiator, respondent, initiator_sk, respondent_sk)
}

fn create_proof(
    env: &Env,
    sk: &ed25519_dalek::SigningKey,
    chain_hash_bytes: &[u8; 32],
    sequence: u64,
) -> RelayChainProof {
    let chain_hash = BytesN::from_array(env, chain_hash_bytes);

    // Sign the raw chain_hash bytes directly as Ed25519 expects the message
    use ed25519_dalek::Signer;
    let sig = sk.sign(chain_hash_bytes.as_slice());
    let signature = BytesN::from_array(env, &sig.to_bytes());

    RelayChainProof {
        signature,
        chain_hash,
        sequence,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn test_resolve_both_valid_initiator_wins() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, initiator, respondent, init_sk, resp_sk) = setup_dispute(&env);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    // Initiator proof: seq 10 (lower = better)
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Respondent proof: seq 15
    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    // Both sigs valid. Initiator seq 10 <= Respondent seq 15. Initiator wins.
    let ruling = client.resolve(&dispute_id);
    assert_eq!(ruling.winner, initiator);
    assert_eq!(ruling.loser, respondent);
}

#[test]
fn test_resolve_both_valid_respondent_wins() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, initiator, respondent, init_sk, resp_sk) = setup_dispute(&env);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    // Initiator proof: seq 20
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 20);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Respondent proof: seq 15 (lower = better)
    let resp_proof = create_proof(&env, &resp_sk, &chain_hash, 15);
    client.respond(&respondent, &dispute_id, &resp_proof);

    // Both sigs valid. Respondent seq 15 < Initiator seq 20. Respondent wins.
    let ruling = client.resolve(&dispute_id);
    assert_eq!(ruling.winner, respondent);
    assert_eq!(ruling.loser, initiator);
}

#[test]
fn test_resolve_initiator_valid_respondent_invalid() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, initiator, respondent, init_sk, _) = setup_dispute(&env);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    // Initiator proof: valid signature
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 20);
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Respondent proof: invalid signature (bad bytes)
    let resp_proof = RelayChainProof {
        signature: BytesN::from_array(&env, &[0u8; 64]), // All zeros is invalid Ed25519
        chain_hash: BytesN::from_array(&env, &chain_hash),
        sequence: 10, // Normally they'd win, but bad sig means they lose
    };
    client.respond(&respondent, &dispute_id, &resp_proof);

    let result = client.try_resolve(&dispute_id);
    // Panics in the try_ invocation result in an error
    assert!(result.is_err());
}

#[test]
fn test_resolve_both_invalid() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, initiator, respondent, _, _) = setup_dispute(&env);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];

    // Initiator proof: invalid
    let init_proof = RelayChainProof {
        signature: BytesN::from_array(&env, &[0u8; 64]),
        chain_hash: BytesN::from_array(&env, &chain_hash),
        sequence: 10,
    };
    let dispute_id = client.raise_dispute(&initiator, &respondent, &tx_id, &init_proof);

    // Respondent proof: invalid
    let resp_proof = RelayChainProof {
        signature: BytesN::from_array(&env, &[0u8; 64]),
        chain_hash: BytesN::from_array(&env, &chain_hash),
        sequence: 15,
    };
    client.respond(&respondent, &dispute_id, &resp_proof);

    let result = client.try_resolve(&dispute_id);
    assert!(result.is_err());
}

#[test]
fn test_raise_dispute_self_rejection() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, initiator, _, init_sk, _) = setup_dispute(&env);

    let tx_id = BytesN::from_array(&env, &[9u8; 32]);
    let chain_hash = [8u8; 32];
    let init_proof = create_proof(&env, &init_sk, &chain_hash, 10);

    let result = client.try_raise_dispute(&initiator, &initiator, &tx_id, &init_proof);
    assert_eq!(result, Err(Ok(ContractError::InvalidRespondent)));
}
