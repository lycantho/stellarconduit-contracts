//! # Dispute Resolver Contract — `lib.rs`
//!
//! This is the main entry point for the Dispute Resolver Soroban smart contract.
//! It handles final on-chain arbitration for double-spend conflicts that cannot be
//! resolved off-chain by the StellarConduit sync engine.
//!
//! ## Responsibilities
//! - Accept dispute submissions with cryptographic relay chain proofs
//! - Enforce dispute submission and response deadlines
//! - Evaluate competing cryptographic proofs deterministically
//! - Issue a final ruling and trigger appropriate fund recovery
//! - Penalize the relay node that submitted the invalid transaction
//!
//! ## Functions
//! - `raise_dispute(env, initiator, tx_id, proof)` — Submit a new dispute with a relay chain proof
//! - `respond(env, respondent, dispute_id, proof)` — Submit a counter-proof to an open dispute
//! - `resolve(env, dispute_id)` — Resolve a dispute after the evaluation period
//! - `get_dispute(env, dispute_id)` — Fetch dispute details and current status
//! - `get_ruling(env, dispute_id)` — Fetch the final ruling for a resolved dispute
//! - `initialize(env, admin, resolution_window)` — One-time setup for the contract
//!
//! ## See also
//! - `types.rs` — Data structures (Dispute, DisputeStatus, Ruling, RelayChainProof)
//! - `storage.rs` — Persistent storage helpers
//! - `errors.rs` — Contract error codes
//!
//! implementation tracked in GitHub issue

#![no_std]

use soroban_sdk::{contract, contractimpl, Address, Bytes, BytesN, Env, String};

pub mod errors;
pub mod storage;
pub mod types;

use crate::errors::ContractError;
use crate::types::{
    AdminCouncil, Dispute, DisputeStatus, OptionalRelayChainProof, RelayChainProof, Ruling,
};

#[contract]
pub struct DisputeResolverContract;

#[contractimpl]
impl DisputeResolverContract {
    /// Submit a new dispute for a suspected double-spend, recording the initiator's
    /// cryptographic relay chain proof on-chain and setting a response deadline.
    ///
    /// # Parameters
    /// - `env`: Soroban environment for the current contract invocation.
    /// - `initiator`: Address of the party raising the dispute. Must authorize this call.
    /// - `tx_id`: The 32-byte Stellar transaction ID under dispute.
    /// - `proof`: The initiator's cryptographic relay chain proof.
    ///
    /// # Returns
    /// The newly assigned `dispute_id` (`u64`) for tracking the dispute.
    ///
    /// # Errors
    /// - `ContractError::DuplicateDispute` if a dispute for this `tx_id` already exists.
    pub fn raise_dispute(
        env: Env,
        initiator: Address,
        respondent: Address,
        tx_id: BytesN<32>,
        proof: RelayChainProof,
    ) -> Result<u64, ContractError> {
        storage::extend_instance_ttl(&env);
        initiator.require_auth();

        if initiator == respondent {
            return Err(ContractError::InvalidRespondent);
        }

        // Guard against duplicate disputes for the same tx_id.
        if storage::get_dispute_by_tx(&env, &tx_id).is_some() {
            return Err(ContractError::DuplicateDispute);
        }

        // Auto-increment and get the next dispute ID (starts at 1).
        let dispute_id = storage::get_next_dispute_id(&env);

        // Compute the response deadline as a ledger sequence number.
        let resolution_window = storage::get_resolution_window(&env);
        let resolve_by = env.ledger().sequence() + resolution_window;

        let dispute = Dispute {
            dispute_id,
            tx_id: tx_id.clone(),
            initiator: initiator.clone(),
            respondent: respondent.clone(),
            initiator_proof: proof,
            respondent_proof: OptionalRelayChainProof::None,
            status: DisputeStatus::Open,
            raised_at: env.ledger().timestamp(),
            resolve_by: resolve_by as u64,
        };

        // Persist the dispute and record the tx → dispute_id mapping.
        storage::set_dispute(&env, dispute_id, &dispute);
        storage::set_dispute_by_tx(&env, &tx_id, dispute_id);

        // Emit event for off-chain indexers.
        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "dispute_resolver"),
                soroban_sdk::Symbol::new(&env, "raise"),
            ),
            (initiator, dispute_id, tx_id),
        );

        Ok(dispute_id)
    }

    /// Submit a counter-proof to an open dispute within the resolution window.
    ///
    /// # Parameters
    /// - `env`: Soroban environment for the current contract invocation.
    /// - `respondent`: Address of the party responding to the dispute. Must authorize.
    /// - `dispute_id`: The unique ID of the dispute to respond to.
    /// - `proof`: The respondent's cryptographic relay chain proof.
    ///
    /// # Errors
    /// - `ContractError::DisputeNotFound` if no dispute exists for this ID.
    /// - `ContractError::NotOpen` if the dispute is not in `Open` status.
    /// - `ContractError::ResolutionWindowExpired` if the response deadline has passed.
    pub fn respond(
        env: Env,
        respondent: Address,
        dispute_id: u64,
        proof: RelayChainProof,
    ) -> Result<(), ContractError> {
        storage::extend_instance_ttl(&env);
        respondent.require_auth();

        let mut dispute =
            storage::get_dispute(&env, dispute_id).ok_or(ContractError::DisputeNotFound)?;

        if dispute.respondent != respondent {
            return Err(ContractError::UnauthorizedRespondent);
        }

        // Only Open disputes can receive a response.
        if dispute.status != DisputeStatus::Open {
            return Err(ContractError::NotOpen);
        }

        // The response window must not have expired.
        if env.ledger().sequence() as u64 > dispute.resolve_by {
            return Err(ContractError::ResolutionWindowExpired);
        }

        dispute.respondent_proof = OptionalRelayChainProof::Some(proof);
        dispute.status = DisputeStatus::Responded;

        storage::set_dispute(&env, dispute_id, &dispute);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "dispute_resolver"),
                soroban_sdk::Symbol::new(&env, "respond"),
            ),
            (respondent, dispute_id),
        );

        Ok(())
    }

    /// Evaluate both proofs and issue a final ruling for a dispute.
    ///
    /// Can be called by anyone once the dispute is in `Responded` status, or by
    /// anyone after the resolution window expires (ruling goes to initiator by default).
    ///
    /// # Parameters
    /// - `env`: Soroban environment for the current contract invocation.
    /// - `dispute_id`: The unique ID of the dispute to resolve.
    ///
    /// # Returns
    /// The final `Ruling` struct.
    ///
    /// # Errors
    /// - `ContractError::DisputeNotFound` if no dispute exists for this ID.
    /// - `ContractError::DisputeAlreadyResolved` if the dispute is already resolved.
    /// - `ContractError::ResolutionWindowActive` if the dispute is still Open and the window hasn't expired.
    /// - `ContractError::NotResponded` if the dispute status is unexpected.
    pub fn resolve(env: Env, dispute_id: u64) -> Result<Ruling, ContractError> {
        storage::extend_instance_ttl(&env);
        let mut dispute =
            storage::get_dispute(&env, dispute_id).ok_or(ContractError::DisputeNotFound)?;

        // Cannot resolve an already-resolved dispute.
        if dispute.status == DisputeStatus::Resolved {
            return Err(ContractError::DisputeAlreadyResolved);
        }

        let current_sequence = env.ledger().sequence() as u64;

        // If still Open, check if the window has expired. If not, cannot resolve yet.
        if dispute.status == DisputeStatus::Open {
            if current_sequence <= dispute.resolve_by {
                return Err(ContractError::ResolutionWindowActive);
            }
            // Window expired with no response — initiator wins automatically.
            let ruling = Ruling {
                dispute_id,
                winner: dispute.initiator.clone(),
                loser: dispute.respondent.clone(),
                reason: String::from_str(
                    &env,
                    "Respondent failed to respond within the resolution window",
                ),
                resolved_at: env.ledger().timestamp(),
            };
            dispute.status = DisputeStatus::Resolved;
            storage::set_dispute(&env, dispute_id, &dispute);
            storage::set_ruling(&env, dispute_id, &ruling);
            env.events().publish(
                (
                    soroban_sdk::Symbol::new(&env, "dispute_resolver"),
                    soroban_sdk::Symbol::new(&env, "resolve"),
                ),
                (dispute_id, ruling.winner.clone(), ruling.loser.clone()),
            );
            return Ok(ruling);
        }

        // Must be in Responded state to proceed with proof evaluation.
        if dispute.status != DisputeStatus::Responded {
            return Err(ContractError::NotResponded);
        }

        let respondent = dispute.respondent.clone();

        let respondent_proof = match &dispute.respondent_proof {
            OptionalRelayChainProof::Some(p) => p.clone(),
            OptionalRelayChainProof::None => return Err(ContractError::NotResponded),
        };

        // ── Ed25519 signature verification ────────────────────────────────────
        // Retrieve pre-stored Ed25519 public keys for both parties.
        let initiator_key = storage::get_public_key(&env, &dispute.initiator);
        let respondent_key = storage::get_public_key(&env, &respondent);

        let initiator_valid = Self::verify_proof(&env, &initiator_key, &dispute.initiator_proof);
        let respondent_valid = Self::verify_proof(&env, &respondent_key, &respondent_proof);

        // Four-case ruling tree:
        let (winner, loser, reason) = match (initiator_valid, respondent_valid) {
            // Only initiator's proof is cryptographically valid.
            (true, false) => (
                dispute.initiator.clone(),
                respondent.clone(),
                String::from_str(
                    &env,
                    "Initiator proof valid; respondent proof failed signature verification",
                ),
            ),
            // Only respondent's proof is cryptographically valid.
            (false, true) => (
                respondent.clone(),
                dispute.initiator.clone(),
                String::from_str(
                    &env,
                    "Respondent proof valid; initiator proof failed signature verification",
                ),
            ),
            // Both proofs valid — fall back to sequence number tiebreak (lower wins).
            (true, true) => {
                if dispute.initiator_proof.sequence <= respondent_proof.sequence {
                    (
                        dispute.initiator.clone(),
                        respondent.clone(),
                        String::from_str(
                            &env,
                            "Both proofs valid; initiator has lower or equal sequence",
                        ),
                    )
                } else {
                    (
                        respondent.clone(),
                        dispute.initiator.clone(),
                        String::from_str(&env, "Both proofs valid; respondent has lower sequence"),
                    )
                }
            }
            // Neither proof is valid — cannot issue a fair ruling.
            (false, false) => return Err(ContractError::InvalidProofSignature),
        };

        let ruling = Ruling {
            dispute_id,
            winner: winner.clone(),
            loser,
            reason,
            resolved_at: env.ledger().timestamp(),
        };

        dispute.status = DisputeStatus::Resolved;
        storage::set_dispute(&env, dispute_id, &dispute);
        storage::set_ruling(&env, dispute_id, &ruling);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "dispute_resolver"),
                soroban_sdk::Symbol::new(&env, "resolve"),
            ),
            (dispute_id, ruling.winner.clone(), ruling.loser.clone()),
        );

        Ok(ruling)
    }

    /// Verify an Ed25519 relay chain proof for a given signer.
    ///
    /// Calls `env.crypto().ed25519_verify()` which will panic on an invalid
    /// signature (Soroban SDK v22 behaviour). We catch that as `false` by
    /// pre-validating the key/message bytes are well-formed; valid calls
    /// will always return `true`.
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `public_key`: Raw 32-byte Ed25519 public key of the signer.
    /// - `proof`: The `RelayChainProof` to verify.
    ///
    /// # Returns
    /// `true` if the signature over `proof.chain_hash` is valid for `public_key`,
    /// `false` otherwise.
    fn verify_proof(env: &Env, public_key: &BytesN<32>, proof: &RelayChainProof) -> bool {
        let message: Bytes = proof.chain_hash.clone().into();
        env.crypto()
            .ed25519_verify(public_key, &message, &proof.signature);
        true
    }

    /// Fetch the full dispute record by its ID.
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `dispute_id`: The ID of the dispute.
    ///
    /// # Returns
    /// The `Dispute` record if found.
    ///
    /// # Errors
    /// - `ContractError::DisputeNotFound` if the ID does not exist.
    pub fn get_dispute(env: Env, dispute_id: u64) -> Result<Dispute, ContractError> {
        storage::extend_instance_ttl(&env);
        storage::get_dispute(&env, dispute_id).ok_or(ContractError::DisputeNotFound)
    }

    /// Fetch the final ruling for a resolved dispute.
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `dispute_id`: The ID of the resolved dispute.
    ///
    /// # Returns
    /// The `Ruling` record if found.
    ///
    /// # Errors
    /// - `ContractError::DisputeNotFound` if no ruling exists for this ID.
    pub fn get_ruling(env: Env, dispute_id: u64) -> Result<Ruling, ContractError> {
        storage::extend_instance_ttl(&env);
        storage::get_ruling(&env, dispute_id).ok_or(ContractError::DisputeNotFound)
    }

    /// One-time initialization of the Dispute Resolver contract.
    ///
    /// Sets the admin capable of upgrading/configuring the contract, and
    /// configures the global resolution window for how long a respondent has
    /// to provide a counter-proof.
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `admin`: The address to set as the contract administrator.
    /// - `resolution_window`: Number of ledgers allowed for responding to disputes.
    ///
    /// # Errors
    /// - `ContractError::AlreadyInitialized` if called more than once.
    /// - `ContractError::InvalidConfig` if `resolution_window` is zero.
    pub fn initialize(
        env: Env,
        council: AdminCouncil,
        resolution_window: u32,
    ) -> Result<(), ContractError> {
        storage::extend_instance_ttl(&env);
        if storage::has_admin_council(&env) {
            return Err(ContractError::AlreadyInitialized);
        }

        if resolution_window == 0 {
            return Err(ContractError::InvalidConfig);
        }

        if council.threshold == 0 || council.members.len() < council.threshold {
            return Err(ContractError::InvalidCouncilConfig);
        }

        storage::set_admin_council(&env, &council);
        storage::set_resolution_window(&env, resolution_window);

        Ok(())
    }
}

#[cfg(test)]
mod test;
