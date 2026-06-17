//! # Dispute Resolver Contract — `types.rs`
//!
//! Defines all data structures used by the Dispute Resolver contract.
//! Purely type definitions — no logic. These structs and enums are used by
//! every function in the contract.

use soroban_sdk::{contracttype, Address, BytesN, String, Vec};

/// A multi-signature admin council requiring threshold approvals.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminCouncil {
    /// List of council member addresses (max 10)
    pub members: Vec<Address>,
    /// Minimum number of members required to authorize a sensitive action
    pub threshold: u32,
}

/// Lifecycle status of a dispute.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisputeStatus {
    /// Dispute raised, awaiting a counter-proof from the respondent.
    Open,
    /// Counter-proof has been submitted, awaiting resolution.
    Responded,
    /// A final ruling has been issued.
    Resolved,
    /// The resolution window passed without a response.
    Expired,
}

/// A cryptographic proof submitted by a party in a dispute.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayChainProof {
    /// Ed25519 signature of the relay chain hash.
    pub signature: BytesN<64>,
    /// Hash of the relay chain at the point of signing.
    pub chain_hash: BytesN<32>,
    /// Sequence number in the relay chain at signing.
    pub sequence: u64,
}

/// Optional relay chain proof; used for respondent's counter-proof before they respond.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionalRelayChainProof {
    /// No proof submitted yet.
    None,
    /// Proof provided.
    Some(RelayChainProof),
}

/// The primary on-chain record for a submitted dispute.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispute {
    /// Unique monotonic identifier for this dispute.
    pub dispute_id: u64,
    /// The Stellar transaction ID under dispute.
    pub tx_id: BytesN<32>,
    /// Address that raised the dispute.
    pub initiator: Address,
    /// Counter-party address.
    pub respondent: Address,
    /// Proof from the initiator.
    pub initiator_proof: RelayChainProof,
    /// Counter-proof from the respondent; set on response.
    pub respondent_proof: OptionalRelayChainProof,
    /// Current lifecycle status of the dispute.
    pub status: DisputeStatus,
    /// Ledger timestamp when the dispute was raised.
    pub raised_at: u64,
    /// Ledger sequence deadline for response.
    pub resolve_by: u64,
}

/// The final arbitration outcome, written only upon resolution.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ruling {
    /// The dispute this ruling belongs to.
    pub dispute_id: u64,
    /// Address that won the dispute.
    pub winner: Address,
    /// Address that lost and will be penalized.
    pub loser: Address,
    /// Brief human-readable explanation of the ruling.
    pub reason: String,
    /// Ledger timestamp when the ruling was issued.
    pub resolved_at: u64,
}
