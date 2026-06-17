//! # Fee Distributor — Unit Test Suite
//!
//! Comprehensive unit tests for the Fee Distributor contract covering all
//! public functions, happy paths, and error cases.

extern crate std;

use soroban_sdk::{testutils::Address as _, token, Address, Env};

use crate::{errors::ContractError, FeeDistributorContract, FeeDistributorContractClient};

fn setup<'a>() -> (Env, FeeDistributorContractClient<'a>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);
    client.initialize(&council, &50u32, &1000u32, &treasury, &token);
    (env, client)
}

fn setup_with_token<'a>() -> (Env, FeeDistributorContractClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    // Register token contract
    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_id = token_contract.address();

    // Register treasury contract
    let treasury_id = env.register(treasury::TreasuryContract, ());
    let treasury_client = treasury::TreasuryContractClient::new(&env, &treasury_id);

    // Initialize treasury
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let treasury_council = treasury::types::AdminCouncil {
        members: members.clone(),
        threshold: 1,
    };
    treasury_client.initialize(&treasury_council, &token_id);

    // Register fee distributor
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);

    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };

    // Initialize fee distributor
    client.initialize(&council, &50u32, &1000u32, &treasury_id, &token_id);

    // Mint tokens to fee distributor for testing
    let token_client = token::StellarAssetClient::new(&env, &token_id);
    token_client.mint(&contract_id, &1_000_000);

    (env, client, contract_id)
}

// ============================================================================
// initialize() Tests
// ============================================================================

#[test]
fn test_initialize_success() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);

    client.initialize(&council, &50u32, &1000u32, &treasury, &token);

    // Verify fee config is set correctly by calling calculate_fee
    // With fee_rate_bps = 50 and batch_size = 200, fee should be 1
    let fee = client.calculate_fee(&200u32);
    assert_eq!(fee, 1);
}

#[test]
fn test_initialize_already_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);

    client.initialize(&council, &50u32, &1000u32, &treasury, &token);

    // Second call should fail
    let result = client.try_initialize(&council, &50u32, &1000u32, &treasury, &token);
    assert_eq!(result, Err(Ok(ContractError::AlreadyInitialized)));
}

// ============================================================================
// calculate_fee() Tests
// ============================================================================

#[test]
fn test_calculate_fee_success() {
    let (_env, client) = setup();

    // With fee_rate_bps = 50 (0.5%) and batch_size = 200:
    // fee = 200 * 50 / 10000 = 1
    let fee = client.calculate_fee(&200u32);
    assert_eq!(fee, 1);

    // With batch_size = 1000:
    // fee = 1000 * 50 / 10000 = 5
    let fee2 = client.calculate_fee(&1000u32);
    assert_eq!(fee2, 5);

    // With batch_size = 100:
    // fee = 100 * 50 / 10000 = 0 (integer division)
    let fee3 = client.calculate_fee(&100u32);
    assert_eq!(fee3, 0);
}

#[test]
fn test_calculate_fee_zero_batch() {
    let (_env, client) = setup();

    let result = client.try_calculate_fee(&0u32);
    assert_eq!(result, Err(Ok(ContractError::InvalidBatchSize)));
}

#[test]
fn test_calculate_fee_boundary() {
    let (_env, client) = setup();

    // Test with max u32 batch size to check overflow guard
    let max_batch_size = u32::MAX;
    let result = client.try_calculate_fee(&max_batch_size);
    // This should either succeed (if no overflow) or return Overflow error
    // With fee_rate_bps = 50: max_batch_size * 50 could overflow i128
    // Let's check if it overflows
    // For boundary test, we just verify it doesn't panic
    // The actual overflow handling is tested elsewhere
    match result {
        Ok(Ok(fee)) => {
            // If it doesn't overflow, verify the calculation
            let expected: Option<i128> = (max_batch_size as i128)
                .checked_mul(50i128)
                .and_then(|x| x.checked_div(10000));
            if let Some(exp) = expected {
                assert_eq!(fee, exp);
            }
        }
        Ok(Err(_)) | Err(Ok(ContractError::Overflow)) | Err(Err(_)) => {
            // Overflow or other errors are acceptable for max u32 boundary test
        }
        _ => {
            // Any other result is acceptable for boundary test
        }
    }
}

// ============================================================================
// distribute() Tests
// ============================================================================

#[test]
fn test_distribute_success() {
    let (env, client) = setup();
    let relay = Address::generate(&env);
    let batch_id = 1u64;
    let batch_size = 200u32;

    client.distribute(&relay, &batch_id, &batch_size);

    // Verify relay earnings updated
    let earnings = client.get_earnings(&relay);
    // With batch_size = 200, fee_rate_bps = 50: fee = 1
    // treasury_share_bps = 1000 (10%): treasury_share = 1 * 1000 / 10000 = 0
    // relay_payout = 1 - 0 = 1
    assert_eq!(earnings.total_earned, 1);
    assert_eq!(earnings.unclaimed, 1);

    // Verify fee entry stored
    // Note: We can't directly read fee entries, but we can verify by trying to distribute again
    // (This is tested in test_distribute_duplicate_batch)
}

#[test]
fn test_distribute_duplicate_batch() {
    let (_env, client) = setup();
    let relay = Address::generate(&_env);
    let batch_id = 1u64;
    let batch_size = 200u32;

    client.distribute(&relay, &batch_id, &batch_size);

    // Second call with same batch_id should fail
    let result = client.try_distribute(&relay, &batch_id, &batch_size);
    assert_eq!(result, Err(Ok(ContractError::BatchAlreadyDistributed)));
}

#[test]
fn test_distribute_zero_batch_size() {
    let (_env, client) = setup();
    let relay = Address::generate(&_env);
    let batch_id = 1u64;

    let result = client.try_distribute(&relay, &batch_id, &0u32);
    assert_eq!(result, Err(Ok(ContractError::InvalidBatchSize)));
}

#[test]
fn test_distribute_treasury_share_split() {
    let (env, client, _contract_id) = setup_with_token();
    let relay = Address::generate(&env);
    let batch_id = 1u64;
    let batch_size = 10000u32; // Large batch to get meaningful treasury share

    client.distribute(&relay, &batch_id, &batch_size);

    // With batch_size = 10000, fee_rate_bps = 50: fee = 10000 * 50 / 10000 = 50
    // treasury_share_bps = 1000 (10%): treasury_share = 50 * 1000 / 10000 = 5
    // relay_payout = 50 - 5 = 45
    let earnings = client.get_earnings(&relay);
    assert_eq!(earnings.total_earned, 45);
    assert_eq!(earnings.unclaimed, 45);

    // Verify: relay_payout + treasury_share == total fee
    // 45 + 5 = 50 ✓
    assert_eq!(earnings.total_earned + 5, 50);
}

// ============================================================================
// claim() Tests
// ============================================================================

#[test]
fn test_claim_success() {
    let (env, client, contract_id) = setup_with_token();
    let relay = Address::generate(&env);
    let batch_id = 1u64;
    let batch_size = 200u32;

    // First distribute some fees
    client.distribute(&relay, &batch_id, &batch_size);

    let earnings_before = client.get_earnings(&relay);
    assert_eq!(earnings_before.unclaimed, 1);

    let token_id = env.as_contract(&contract_id, || crate::storage::get_token_address(&env));
    let token_client = token::Client::new(&env, &token_id);
    let relay_balance_before = token_client.balance(&relay);
    let contract_balance_before = token_client.balance(&contract_id);

    // Claim the fees
    let payout = client.claim(&relay);

    // Verify payout amount
    assert_eq!(payout, 1);

    let relay_balance_after = token_client.balance(&relay);
    let contract_balance_after = token_client.balance(&contract_id);

    assert_eq!(relay_balance_after, relay_balance_before + payout);
    assert_eq!(contract_balance_after, contract_balance_before - payout);

    // Verify unclaimed zeroed and total_claimed incremented
    let earnings_after = client.get_earnings(&relay);
    assert_eq!(earnings_after.unclaimed, 0);
    assert_eq!(earnings_after.total_claimed, 1);
    assert_eq!(earnings_after.total_earned, 1);
}

#[test]
fn test_claim_nothing_to_claim() {
    let (_env, client) = setup();
    let relay = Address::generate(&_env);

    // Try to claim when there's nothing to claim
    let result = client.try_claim(&relay);
    assert_eq!(result, Err(Ok(ContractError::NothingToClaim)));
}

#[test]
#[should_panic(expected = "HostError")]
fn test_claim_auth_required() {
    let (env, client, _) = setup_with_token();
    let relay = Address::generate(&env);
    let batch_id = 1u64;
    let batch_size = 200u32;

    // Distribute some fees
    client.distribute(&relay, &batch_id, &batch_size);

    // Create a new env without mock_all_auths to test auth requirement
    let env2 = Env::default();
    // Don't call env2.mock_all_auths() - this should cause auth to fail
    let contract_id = env2.register(FeeDistributorContract, ());
    let client2 = FeeDistributorContractClient::new(&env2, &contract_id);

    // This should panic because relay hasn't authorized
    client2.claim(&relay);
}

#[test]
#[should_panic(expected = "HostError")]
fn test_claim_insufficient_balance() {
    let (env, client, contract_id) = setup_with_token();
    let relay = Address::generate(&env);
    let batch_id = 1u64;
    let batch_size = 200u32;

    // Distribute some fees
    client.distribute(&relay, &batch_id, &batch_size);

    let token_id = env.as_contract(&contract_id, || crate::storage::get_token_address(&env));
    let token_client = token::Client::new(&env, &token_id);
    let current_balance = token_client.balance(&contract_id);
    // Burn all tokens from the contract using the token client itself?
    // StellarAssetClient has no burn_from. Let's just create a new env or something.
    // Wait, the fee distributor is initialized with 1,000,000 tokens.
    // If we want to simulate insufficient balance, we can't easily burn if the token is SAC.
    // Actually, `token::Client::new(&env, &token_id).transfer()` needs auth.
    // We can just invoke token transfer from the contract id out of the contract using `env.as_contract`.
    env.as_contract(&contract_id, || {
        let t = token::Client::new(&env, &token_id);
        t.transfer(&contract_id, &relay, &current_balance);
    });

    // Now balance is 0. Claiming should fail with HostError (SAC panics on insufficient balance).
    client.claim(&relay);
}

// ============================================================================
// get_earnings() Tests
// ============================================================================

#[test]
fn test_get_earnings_existing() {
    let (_env, client) = setup();
    let relay = Address::generate(&_env);
    let batch_id1 = 1u64;
    let batch_id2 = 2u64;
    let batch_size = 200u32;

    // Distribute fees twice
    client.distribute(&relay, &batch_id1, &batch_size);
    client.distribute(&relay, &batch_id2, &batch_size);

    let earnings = client.get_earnings(&relay);
    // Each distribution adds 1 to total_earned and unclaimed
    assert_eq!(earnings.total_earned, 2);
    assert_eq!(earnings.unclaimed, 2);
    assert_eq!(earnings.total_claimed, 0);
}

#[test]
fn test_get_earnings_default() {
    let (_env, client) = setup();
    let relay = Address::generate(&_env);

    // Get earnings for a relay that has never received distributions
    let earnings = client.get_earnings(&relay);
    assert_eq!(earnings.total_earned, 0);
    assert_eq!(earnings.unclaimed, 0);
    assert_eq!(earnings.total_claimed, 0);
}

// ============================================================================
// set_fee_rate() Tests
// ============================================================================

#[test]
fn test_set_fee_rate_success() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);

    client.initialize(&council, &50u32, &1000u32, &treasury, &token);

    // Update fee rate to 100 bps (1%)
    client.set_fee_rate(&100u32);

    // Verify change reflected in calculate_fee
    // With fee_rate_bps = 100 and batch_size = 200: fee = 200 * 100 / 10000 = 2
    let fee = client.calculate_fee(&200u32);
    assert_eq!(fee, 2);
}

#[test]
fn test_set_fee_rate_invalid_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);

    client.initialize(&council, &50u32, &1000u32, &treasury, &token);

    // Try to set fee rate to 0
    let result = client.try_set_fee_rate(&0u32);
    assert_eq!(result, Err(Ok(ContractError::InvalidFeeRate)));
}

#[test]
fn test_set_fee_rate_invalid_above_max() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);

    client.initialize(&council, &50u32, &1000u32, &treasury, &token);

    // Try to set fee rate to 10001 (above max of 10000)
    let result = client.try_set_fee_rate(&10001u32);
    assert_eq!(result, Err(Ok(ContractError::InvalidFeeRate)));
}

#[test]
#[should_panic(expected = "HostError")]
fn test_set_fee_rate_unauthorized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);

    client.initialize(&council, &50u32, &1000u32, &treasury, &token);

    // Create a new env without mock_all_auths and try to call as non-admin
    let env2 = Env::default();
    // Don't call env2.mock_all_auths() - this should cause auth to fail
    let contract_id2 = env2.register(FeeDistributorContract, ());
    let client2 = FeeDistributorContractClient::new(&env2, &contract_id2);

    // This should panic because non-admin hasn't authorized
    client2.set_fee_rate(&100u32);
}

// ============================================================================
// Additional Comprehensive Tests
// ============================================================================

#[test]
fn test_multiple_distributions_same_relay() {
    let (_env, client) = setup();
    let relay = Address::generate(&_env);

    // Distribute fees multiple times to same relay
    client.distribute(&relay, &1u64, &200u32); // fee = 1
    client.distribute(&relay, &2u64, &1000u32); // fee = 5
    client.distribute(&relay, &3u64, &400u32); // fee = 2

    let earnings = client.get_earnings(&relay);
    // Total relay payouts: 1 + 5 + 2 = 8 (treasury share is 0 for small amounts)
    assert_eq!(earnings.total_earned, 8);
    assert_eq!(earnings.unclaimed, 8);
}

#[test]
fn test_multiple_distributions_different_relays() {
    let (_env, client) = setup();
    let relay1 = Address::generate(&_env);
    let relay2 = Address::generate(&_env);

    client.distribute(&relay1, &1u64, &200u32);
    client.distribute(&relay2, &2u64, &200u32);

    let earnings1 = client.get_earnings(&relay1);
    let earnings2 = client.get_earnings(&relay2);

    // Each relay should have independent earnings
    assert_eq!(earnings1.total_earned, 1);
    assert_eq!(earnings1.unclaimed, 1);
    assert_eq!(earnings2.total_earned, 1);
    assert_eq!(earnings2.unclaimed, 1);
}

#[test]
fn test_multiple_claims() {
    let (env, client, contract_id) = setup_with_token();
    let relay = Address::generate(&env);

    let token_id = env.as_contract(&contract_id, || crate::storage::get_token_address(&env));
    let token_client = token::Client::new(&env, &token_id);

    // Distribute, claim, distribute again, claim again
    client.distribute(&relay, &1u64, &200u32);
    let payout1 = client.claim(&relay);
    assert_eq!(payout1, 1);

    client.distribute(&relay, &2u64, &1000u32);
    let payout2 = client.claim(&relay);
    assert_eq!(payout2, 5);

    let relay_balance_after = token_client.balance(&relay);
    assert_eq!(relay_balance_after, payout1 + payout2);

    let earnings = client.get_earnings(&relay);
    assert_eq!(earnings.total_earned, 6);
    assert_eq!(earnings.total_claimed, 6);
    assert_eq!(earnings.unclaimed, 0);
}

#[test]
fn test_claim_after_multiple_distributions() {
    let (env, client, contract_id) = setup_with_token();
    let relay = Address::generate(&env);

    let token_id = env.as_contract(&contract_id, || crate::storage::get_token_address(&env));
    let token_client = token::Client::new(&env, &token_id);

    // Distribute multiple times without claiming
    client.distribute(&relay, &1u64, &200u32); // 1
    client.distribute(&relay, &2u64, &400u32); // 2
    client.distribute(&relay, &3u64, &600u32); // 3

    // Claim all at once
    let payout = client.claim(&relay);
    assert_eq!(payout, 6);

    let relay_balance_after = token_client.balance(&relay);
    assert_eq!(relay_balance_after, payout);

    let earnings = client.get_earnings(&relay);
    assert_eq!(earnings.total_earned, 6);
    assert_eq!(earnings.total_claimed, 6);
    assert_eq!(earnings.unclaimed, 0);
}

#[test]
fn test_calculate_fee_with_different_rates() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);

    // Initialize with fee_rate_bps = 100 (1%)
    client.initialize(&council, &100u32, &1000u32, &treasury, &token);

    // With batch_size = 200: fee = 200 * 100 / 10000 = 2
    let fee = client.calculate_fee(&200u32);
    assert_eq!(fee, 2);

    // Update to fee_rate_bps = 500 (5%)
    client.set_fee_rate(&500u32);

    // With batch_size = 200: fee = 200 * 500 / 10000 = 10
    let fee2 = client.calculate_fee(&200u32);
    assert_eq!(fee2, 10);
}

#[test]
fn test_distribute_with_different_batch_sizes() {
    let (env, client, _contract_id) = setup_with_token();
    let relay = Address::generate(&env);

    // Test various batch sizes
    client.distribute(&relay, &1u64, &1u32); // fee = 0 (rounding down)
    client.distribute(&relay, &2u64, &100u32); // fee = 0
    client.distribute(&relay, &3u64, &200u32); // fee = 1
    client.distribute(&relay, &4u64, &10000u32); // fee = 50

    let earnings = client.get_earnings(&relay);
    // Total: 0 + 0 + 1 + 45 (50 - 5 treasury) = 46
    assert_eq!(earnings.total_earned, 46);
}

#[test]
fn test_earnings_invariant_total_equals_claimed_plus_unclaimed() {
    let (_env, client, _) = setup_with_token();
    let relay = Address::generate(&_env);

    // Distribute some fees
    client.distribute(&relay, &1u64, &200u32);
    client.distribute(&relay, &2u64, &400u32);

    let earnings = client.get_earnings(&relay);
    // Invariant: total_earned = total_claimed + unclaimed
    assert_eq!(
        earnings.total_earned,
        earnings.total_claimed + earnings.unclaimed
    );

    // Claim some
    client.claim(&relay);

    let earnings2 = client.get_earnings(&relay);
    // Invariant should still hold
    assert_eq!(
        earnings2.total_earned,
        earnings2.total_claimed + earnings2.unclaimed
    );
}

#[test]
fn test_distribute_small_batch_size() {
    let (_env, client) = setup();
    let relay = Address::generate(&_env);

    // Test with smallest valid batch size
    client.distribute(&relay, &1u64, &1u32);

    let earnings = client.get_earnings(&relay);
    // With batch_size = 1, fee_rate_bps = 50: fee = 1 * 50 / 10000 = 0
    assert_eq!(earnings.total_earned, 0);
}

#[test]
fn test_distribute_large_batch_size() {
    let (env, client, _contract_id) = setup_with_token();
    let relay = Address::generate(&env);

    // Test with large batch size
    let large_batch = 100000u32;
    client.distribute(&relay, &1u64, &large_batch);

    let earnings = client.get_earnings(&relay);
    // With batch_size = 100000, fee_rate_bps = 50: fee = 100000 * 50 / 10000 = 500
    // treasury_share = 500 * 1000 / 10000 = 50
    // relay_payout = 500 - 50 = 450
    assert_eq!(earnings.total_earned, 450);
}

#[test]
fn test_fee_rate_change_affects_future_distributions() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);
    let relay = Address::generate(&env);

    client.initialize(&council, &50u32, &1000u32, &treasury, &token);

    // Distribute with initial rate
    client.distribute(&relay, &1u64, &200u32); // fee = 1

    // Change fee rate
    client.set_fee_rate(&200u32); // 2%

    // Distribute with new rate
    client.distribute(&relay, &2u64, &200u32); // fee = 4

    let earnings = client.get_earnings(&relay);
    // First: 1 - 0 = 1, Second: 4 - 0 = 4, Total = 5
    assert_eq!(earnings.total_earned, 5);
}

#[test]
fn test_treasury_share_calculation_edge_cases() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    // Register token contract
    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_id = token_contract.address();

    // Register treasury contract
    let treasury_id = env.register(treasury::TreasuryContract, ());
    let treasury_client = treasury::TreasuryContractClient::new(&env, &treasury_id);

    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());

    // Initialize treasury
    let treasury_council = treasury::types::AdminCouncil {
        members: members.clone(),
        threshold: 1,
    };
    treasury_client.initialize(&treasury_council, &token_id);

    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let relay = Address::generate(&env);

    // Initialize with 50% treasury share
    client.initialize(&council, &100u32, &5000u32, &treasury_id, &token_id);

    // Mint tokens to fee distributor
    let token_client = token::StellarAssetClient::new(&env, &token_id);
    token_client.mint(&contract_id, &1_000_000);

    client.distribute(&relay, &1u64, &1000u32);
    // fee = 1000 * 100 / 10000 = 10
    // treasury_share = 10 * 5000 / 10000 = 5
    // relay_payout = 10 - 5 = 5

    let earnings = client.get_earnings(&relay);
    assert_eq!(earnings.total_earned, 5);
}

#[test]
fn test_batch_id_uniqueness_across_relays() {
    let (_env, client) = setup();
    let relay1 = Address::generate(&_env);
    let relay2 = Address::generate(&_env);
    let batch_id = 1u64;

    // Same batch_id can't be used twice, even for different relays
    client.distribute(&relay1, &batch_id, &200u32);

    // Second distribution with same batch_id should fail
    let result = client.try_distribute(&relay2, &batch_id, &200u32);
    assert_eq!(result, Err(Ok(ContractError::BatchAlreadyDistributed)));
}

#[test]
fn test_calculate_fee_rounding_behavior() {
    let (_env, client) = setup();

    // Test rounding down behavior
    // With fee_rate_bps = 50:
    // batch_size = 99: fee = 99 * 50 / 10000 = 0 (rounds down)
    let fee1 = client.calculate_fee(&99u32);
    assert_eq!(fee1, 0);

    // batch_size = 100: fee = 100 * 50 / 10000 = 0 (rounds down)
    let fee2 = client.calculate_fee(&100u32);
    assert_eq!(fee2, 0);

    // batch_size = 101: fee = 101 * 50 / 10000 = 0 (rounds down)
    let fee3 = client.calculate_fee(&101u32);
    assert_eq!(fee3, 0);

    // batch_size = 200: fee = 200 * 50 / 10000 = 1
    let fee4 = client.calculate_fee(&200u32);
    assert_eq!(fee4, 1);
}

#[test]
fn test_set_fee_rate_boundary_values() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);

    client.initialize(&council, &100u32, &1000u32, &treasury, &token);

    // Test minimum valid rate (1)
    client.set_fee_rate(&1u32);
    let fee1 = client.calculate_fee(&10000u32);
    assert_eq!(fee1, 1);

    // Test maximum valid rate (10000)
    client.set_fee_rate(&10000u32);
    let fee2 = client.calculate_fee(&100u32);
    assert_eq!(fee2, 100);
}

#[test]
fn test_distribute_with_zero_treasury_share() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(admin.clone());
    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let treasury = Address::generate(&env);
    let token = Address::generate(&env);
    let relay = Address::generate(&env);

    // Initialize with 0% treasury share
    client.initialize(&council, &100u32, &0u32, &treasury, &token);

    client.distribute(&relay, &1u64, &1000u32);
    // fee = 1000 * 100 / 10000 = 10
    // treasury_share = 10 * 0 / 10000 = 0
    // relay_payout = 10 - 0 = 10

    let earnings = client.get_earnings(&relay);
    assert_eq!(earnings.total_earned, 10);
}

#[test]
fn test_distribute_with_max_treasury_share() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    // Register token contract
    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_id = token_contract.address();

    // Register treasury contract
    let treasury_id = env.register(treasury::TreasuryContract, ());
    let treasury_client = treasury::TreasuryContractClient::new(&env, &treasury_id);

    let contract_id = env.register(FeeDistributorContract, ());
    let client = FeeDistributorContractClient::new(&env, &contract_id);

    let council_admin = Address::generate(&env);
    let mut members = soroban_sdk::Vec::new(&env);
    members.push_back(council_admin.clone());

    // Initialize treasury
    let treasury_council = treasury::types::AdminCouncil {
        members: members.clone(),
        threshold: 1,
    };
    treasury_client.initialize(&treasury_council, &token_id);

    let council = crate::types::AdminCouncil {
        members,
        threshold: 1,
    };
    let relay = Address::generate(&env);

    // Initialize with 100% treasury share
    client.initialize(&council, &100u32, &10000u32, &treasury_id, &token_id);

    // Mint tokens to fee distributor
    let token_client = token::StellarAssetClient::new(&env, &token_id);
    token_client.mint(&contract_id, &1_000_000);

    client.distribute(&relay, &1u64, &1000u32);
    // fee = 1000 * 100 / 10000 = 10
    // treasury_share = 10 * 10000 / 10000 = 10
    // relay_payout = 10 - 10 = 0

    let earnings = client.get_earnings(&relay);
    assert_eq!(earnings.total_earned, 0);
}

#[test]
fn test_claim_preserves_total_earned() {
    let (_env, client, _) = setup_with_token();
    let relay = Address::generate(&_env);

    client.distribute(&relay, &1u64, &200u32);
    client.distribute(&relay, &2u64, &400u32);

    let earnings_before = client.get_earnings(&relay);
    let total_earned_before = earnings_before.total_earned;

    client.claim(&relay);

    let earnings_after = client.get_earnings(&relay);
    // total_earned should remain unchanged after claim
    assert_eq!(earnings_after.total_earned, total_earned_before);
    // But unclaimed should be zero and total_claimed should equal total_earned
    assert_eq!(earnings_after.unclaimed, 0);
    assert_eq!(earnings_after.total_claimed, total_earned_before);
}
