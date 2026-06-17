//! # Fee Distributor Contract — `lib.rs`
//!
//! This is the main entry point for the Fee Distributor Soroban smart contract.
//! It exposes the public contract interface for protocol fee calculation and
//! distribution to relay nodes upon successful transaction settlement.
//!
//! ## Responsibilities
//! - Calculate relay fee based on batch size and transaction count
//! - Distribute fees to relay nodes upon confirmed settlement on Stellar
//! - Allocate a protocol treasury share from collected fees
//! - Track cumulative fee earnings per relay node
//! - Handle delayed fee claims for relay nodes
//!
//! ## Functions to implement
//! - `distribute(env, relay_address, batch_id)` — Distribute fee for a settled transaction batch
//! - `calculate_fee(env, batch_size)` — Calculate the fee for a given batch of transactions
//! - `claim(env, relay_address)` — Claim accumulated, unclaimed fees for a relay node
//! - `get_earnings(env, relay_address)` — View total lifetime earnings for a relay node
//! - `set_fee_rate(env, rate)` — Update the protocol fee rate (governance-only)
//!
//! ## See also
//! - `types.rs` — Data structures (FeeEntry, EarningsRecord, FeeConfig)
//! - `storage.rs` — Persistent storage helpers
//! - `errors.rs` — Contract error codes
//!
//! implementation tracked in GitHub issue

#![no_std]

use soroban_sdk::{contract, contractimpl, token, Address, Env, IntoVal};

pub mod errors;
pub mod storage;
pub mod types;

#[cfg(test)]
mod test;

use crate::errors::ContractError;
use crate::types::AdminCouncil;

fn require_council_auth(_env: &Env, council: &AdminCouncil) {
    let mut authorized = 0u32;
    for member in council.members.iter() {
        member.require_auth();
        authorized += 1;
        if authorized >= council.threshold {
            break;
        }
    }

    if authorized < council.threshold {
        panic!("Insufficient approvals");
    }
}

#[contract]
pub struct FeeDistributorContract;

#[contractimpl]
impl FeeDistributorContract {
    /// Initialize the contract with admin address, fee rate, treasury share, treasury address, and token address.
    ///
    /// This is a one-time setup function called immediately after the contract is deployed.
    /// It sets the admin address, fee rate, treasury share percentage, treasury address, and token address.
    /// It can only be called once.
    ///
    /// # Parameters
    /// - `env`: Soroban environment for the current contract invocation.
    /// - `council`: Admin council authorized to update fee config.
    /// - `fee_rate_bps`: Fee rate in basis points (e.g., 50 = 0.5%). Must be between 1 and 10000.
    /// - `treasury_share_bps`: Treasury's share of each distribution in basis points (e.g., 1000 = 10%).
    /// - `treasury`: Address of the treasury contract that receives the treasury share.
    /// - `token`: Address of the token contract used for fee payments.
    ///
    /// # Errors
    /// - `ContractError::AlreadyInitialized` if the contract has already been initialized.
    pub fn initialize(
        env: Env,
        council: AdminCouncil,
        fee_rate_bps: u32,
        treasury_share_bps: u32,
        treasury: Address,
        token: Address,
    ) -> Result<(), ContractError> {
        storage::extend_instance_ttl(&env);
        // Guard against re-initialization
        if env.storage().instance().has(&storage::DataKey::FeeConfig) {
            return Err(ContractError::AlreadyInitialized);
        }

        // Validate fee rate
        if fee_rate_bps == 0 || fee_rate_bps > 10_000 {
            return Err(ContractError::InvalidFeeRate);
        }

        if council.threshold == 0 || council.members.len() < council.threshold {
            return Err(ContractError::InvalidCouncilConfig);
        }

        // Persist config
        let config = crate::types::FeeConfig {
            fee_rate_bps,
            treasury_share_bps,
            council: council.clone(),
        };
        storage::set_fee_config(&env, &config);
        storage::set_treasury_address(&env, &treasury);
        storage::set_token_address(&env, &token);

        Ok(())
    }

    /// Calculate the total fee for a given batch of transactions.
    ///
    /// This is a pure calculation function that reads the configured fee rate
    /// and returns the total fee amount. No storage is written.
    ///
    /// # Formula
    /// `fee = (batch_size as i128) * (fee_rate_bps as i128) / 10000`
    ///
    /// # Example
    /// - With `fee_rate_bps = 50` (0.5%) and `batch_size = 200`:
    ///   `fee = 200 * 50 / 10000 = 1`
    /// - With `fee_rate_bps = 500` (5%) and `batch_size = 1000`:
    ///   `fee = 1000 * 500 / 10000 = 50`
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `batch_size`: Number of transactions in the settled batch.
    ///
    /// # Errors
    /// - `ContractError::InvalidBatchSize` if `batch_size` is zero.
    /// - `ContractError::Overflow` if the calculation overflows.
    pub fn calculate_fee(env: Env, batch_size: u32) -> Result<i128, ContractError> {
        storage::extend_instance_ttl(&env);
        if batch_size == 0 {
            return Err(ContractError::InvalidBatchSize);
        }

        let config = storage::get_fee_config(&env);

        let total = (batch_size as i128)
            .checked_mul(config.fee_rate_bps as i128)
            .ok_or(ContractError::Overflow)?;

        let fee = total.checked_div(10000).ok_or(ContractError::Overflow)?;

        Ok(fee)
    }

    /// Distribute the fee for a successfully settled transaction batch.
    ///
    /// This function calculates the fee, credits the relay node's earnings,
    /// allocates the protocol treasury share, and permanently records the
    /// distribution event. The treasury share is automatically transferred
    /// to the treasury contract via cross-contract call.
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `relay_address`: Address of the relay node that settled the batch.
    /// - `batch_id`: Unique identifier of the settled transaction batch.
    /// - `batch_size`: Number of transactions in the batch.
    ///
    /// # Errors
    /// - `ContractError::BatchAlreadyDistributed` if `batch_id` has already been processed.
    /// - `ContractError::InvalidBatchSize` if `batch_size` is zero.
    /// - `ContractError::Overflow` if fee/split calculation overflows.
    /// - `ContractError::TreasuryTransferFailed` if the treasury deposit fails.
    pub fn distribute(
        env: Env,
        relay_address: Address,
        batch_id: u64,
        batch_size: u32,
    ) -> Result<(), ContractError> {
        storage::extend_instance_ttl(&env);
        if storage::get_fee_entry(&env, batch_id).is_some() {
            return Err(ContractError::BatchAlreadyDistributed);
        }

        let fee = Self::calculate_fee(env.clone(), batch_size)?;
        let config = storage::get_fee_config(&env);

        let treasury_share = fee
            .checked_mul(config.treasury_share_bps as i128)
            .ok_or(ContractError::Overflow)?
            .checked_div(10000)
            .ok_or(ContractError::Overflow)?;

        let relay_payout = fee
            .checked_sub(treasury_share)
            .ok_or(ContractError::Overflow)?;

        let mut record = storage::get_earnings(&env, &relay_address);
        record.total_earned = record
            .total_earned
            .checked_add(relay_payout)
            .ok_or(ContractError::Overflow)?;
        record.unclaimed = record
            .unclaimed
            .checked_add(relay_payout)
            .ok_or(ContractError::Overflow)?;

        storage::set_earnings(&env, &relay_address, &record);

        let entry = crate::types::FeeEntry {
            batch_id,
            relay_address: relay_address.clone(),
            amount: fee,
            treasury_share,
            settled_at: env.ledger().timestamp(),
        };
        storage::set_fee_entry(&env, batch_id, &entry);

        // Transfer treasury share to treasury contract
        if treasury_share > 0 {
            let treasury_addr = storage::get_treasury_address(&env);
            let token_addr = storage::get_token_address(&env);
            let token_client = token::Client::new(&env, &token_addr);

            token_client.transfer(
                &env.current_contract_address(),
                &treasury_addr,
                &treasury_share,
            );

            // Call treasury.deposit() via cross-contract invocation
            env.invoke_contract::<()>(
                &treasury_addr,
                &soroban_sdk::Symbol::new(&env, "deposit"),
                soroban_sdk::vec![
                    &env,
                    env.current_contract_address().into_val(&env),
                    treasury_share.into_val(&env)
                ],
            );
        }

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "fee_distributor"),
                soroban_sdk::Symbol::new(&env, "distribute"),
            ),
            (
                relay_address.clone(),
                batch_id,
                relay_payout,
                treasury_share,
            ),
        );

        Ok(())
    }

    /// Claim accumulated, unclaimed fees for a relay node.
    ///
    /// This allows a relay node to withdraw all its accumulated, unclaimed fees
    /// to its own address. Upon successful claim, the unclaimed balance is reset
    /// to zero, and the total claimed amount is increased.
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `relay_address`: Address of the relay node claiming its fees. Must authorize the call.
    ///
    /// # Returns
    /// The total amount of fees claimed in this transaction.
    ///
    /// # Errors
    /// - `ContractError::NothingToClaim` if the relay node has no unclaimed earnings.
    /// - `ContractError::Overflow` if the arithmetic for updating `total_claimed` overflows.
    pub fn claim(env: Env, relay_address: Address) -> Result<i128, ContractError> {
        storage::extend_instance_ttl(&env);
        relay_address.require_auth();

        let mut record = storage::get_earnings(&env, &relay_address);

        if record.unclaimed == 0 {
            return Err(ContractError::NothingToClaim);
        }

        let payout = record.unclaimed;

        record.total_claimed = record
            .total_claimed
            .checked_add(payout)
            .ok_or(ContractError::Overflow)?;
        record.unclaimed = 0;

        storage::set_earnings(&env, &relay_address, &record);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "fee_distributor"),
                soroban_sdk::Symbol::new(&env, "claim"),
            ),
            (relay_address.clone(), payout),
        );

        let token_addr = storage::get_token_address(&env);
        let token_client = token::Client::new(&env, &token_addr);

        token_client.transfer(&env.current_contract_address(), &relay_address, &payout);

        Ok(payout)
    }

    /// Retrieve the cumulative earnings record for a relay node.
    ///
    /// This is a read-only view function that returns the total earned,
    /// total claimed, and currently unclaimed fees for the given relay node.
    /// If the node has no earnings history, it returns a zeroed record.
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `relay_address`: Address of the relay node.
    ///
    /// # Returns
    /// An `EarningsRecord` containing the relay node's fee history.
    pub fn get_earnings(env: Env, relay_address: Address) -> crate::types::EarningsRecord {
        storage::extend_instance_ttl(&env);
        storage::get_earnings(&env, &relay_address)
    }

    /// Update the protocol fee rate.
    ///
    /// This function can only be called by the configured admin address.
    /// The fee rate is specified in basis points (bps), where 10000 = 100%.
    ///
    /// # Parameters
    /// - `env`: Soroban environment.
    /// - `new_fee_rate_bps`: The new fee rate in basis points (1 to 10000).
    ///
    /// # Errors
    /// - Auth error if caller is not the admin.
    /// - `ContractError::InvalidFeeRate` if the rate is 0 or greater than 10000.
    pub fn set_fee_rate(env: Env, new_fee_rate_bps: u32) -> Result<(), ContractError> {
        storage::extend_instance_ttl(&env);
        let mut config = storage::get_fee_config(&env);

        require_council_auth(&env, &config.council);

        if new_fee_rate_bps == 0 || new_fee_rate_bps > 10_000 {
            return Err(ContractError::InvalidFeeRate);
        }

        config.fee_rate_bps = new_fee_rate_bps;
        storage::set_fee_config(&env, &config);

        env.events().publish(
            (
                soroban_sdk::Symbol::new(&env, "fee_distributor"),
                soroban_sdk::Symbol::new(&env, "set_fee_rate"),
            ),
            (new_fee_rate_bps,),
        );

        Ok(())
    }
}
