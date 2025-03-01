use radix_engine::ledger::TypedInMemorySubstateStore;
use scrypto::core::NetworkDefinition;
use scrypto::prelude::*;
use scrypto_unit::*;
use transaction::builder::ManifestBuilder;
use transaction::signing::EcdsaPrivateKey;

fn test_dynamic_auth(
    num_keys: usize,
    initial_auth: usize,
    update_auth: Option<usize>,
    signer_public_keys: &[usize],
    should_succeed: bool,
) {
    // Arrange
    let mut store = TypedInMemorySubstateStore::with_bootstrap();
    let mut test_runner = TestRunner::new(true, &mut store);
    let key_and_addresses: Vec<(EcdsaPublicKey, EcdsaPrivateKey, NonFungibleAddress)> = (0
        ..num_keys)
        .map(|_| test_runner.new_key_pair_with_auth_address())
        .collect();
    let addresses: Vec<NonFungibleAddress> = key_and_addresses
        .iter()
        .map(|(_, _, addr)| addr.clone())
        .collect();
    let public_keys: Vec<EcdsaPublicKey> = signer_public_keys
        .iter()
        .map(|index| key_and_addresses.get(*index).unwrap().0)
        .collect();

    let package = test_runner.extract_and_publish_package("component");
    let manifest1 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_function(
            package,
            "AuthComponent",
            "create_component",
            args!(addresses.get(initial_auth).unwrap().clone()),
        )
        .build();
    let receipt1 = test_runner.execute_manifest(manifest1, vec![]);
    receipt1.expect_commit_success();
    let component = receipt1
        .expect_commit()
        .entity_changes
        .new_component_addresses[0];

    if let Some(next_auth) = update_auth {
        let update_manifest = ManifestBuilder::new(&NetworkDefinition::local_simulator())
            .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
            .call_method(
                component,
                "update_auth",
                args!(addresses.get(next_auth).unwrap().clone()),
            )
            .build();
        test_runner
            .execute_manifest(update_manifest, vec![])
            .expect_commit_success();
    }

    // Act
    let manifest2 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_method(component, "get_secret", args!())
        .build();
    let receipt2 = test_runner.execute_manifest(manifest2, public_keys.to_vec());

    // Assert
    if should_succeed {
        receipt2.expect_commit_success();
    } else {
        receipt2.expect_specific_failure(is_auth_error);
    }
}

fn test_dynamic_authlist(
    list_size: usize,
    auth_rule: AccessRule,
    signer_public_keys: &[usize],
    should_succeed: bool,
) {
    let mut store = TypedInMemorySubstateStore::with_bootstrap();
    let mut test_runner = TestRunner::new(true, &mut store);
    let key_and_addresses: Vec<(EcdsaPublicKey, EcdsaPrivateKey, NonFungibleAddress)> = (0
        ..list_size)
        .map(|_| test_runner.new_key_pair_with_auth_address())
        .collect();
    let list: Vec<NonFungibleAddress> = key_and_addresses
        .iter()
        .map(|(_, _, addr)| addr.clone())
        .collect();
    let public_keys: Vec<EcdsaPublicKey> = signer_public_keys
        .iter()
        .map(|index| key_and_addresses.get(*index).unwrap().0)
        .collect();
    let authorization = AccessRules::new().method("get_secret", auth_rule);

    // Arrange
    let package = test_runner.extract_and_publish_package("component");
    let manifest1 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_function(
            package,
            "AuthListComponent",
            "create_component",
            args!(2u8, list, authorization),
        )
        .build();
    let receipt0 = test_runner.execute_manifest(manifest1, vec![]);
    receipt0.expect_commit_success();
    let component = receipt0
        .expect_commit()
        .entity_changes
        .new_component_addresses[0];

    // Act
    let manifest2 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_method(component, "get_secret", args!())
        .build();
    let receipt = test_runner.execute_manifest(manifest2, public_keys.to_vec());

    // Assert
    if should_succeed {
        receipt.expect_commit_success();
    } else {
        receipt.expect_specific_failure(is_auth_error);
    }
}

#[test]
fn dynamic_auth_should_allow_me_to_call_method_when_signed() {
    test_dynamic_auth(1, 0, None, &[0], true);
}

#[test]
fn dynamic_auth_should_not_allow_me_to_call_method_when_signed_by_another_key() {
    test_dynamic_auth(2, 0, None, &[1], false);
}

#[test]
fn dynamic_auth_should_not_allow_me_to_call_method_when_change_auth() {
    test_dynamic_auth(2, 0, Some(1), &[0], false);
}

#[test]
fn dynamic_auth_should_allow_me_to_call_method_when_change_auth() {
    test_dynamic_auth(2, 0, Some(1), &[1], true);
}

#[test]
fn dynamic_require_should_fail_on_dynamic_list() {
    test_dynamic_authlist(3, rule!(require("auth")), &[0, 1, 2], false);
}

#[test]
fn dynamic_all_of_should_fail_on_nonexistent_resource() {
    test_dynamic_authlist(3, rule!(require("does_not_exist")), &[0, 1, 2], false);
}

#[test]
fn dynamic_min_n_of_should_allow_me_to_call_method() {
    let auths = [
        rule!(require_n_of(2, "auth")),
        rule!(require_n_of("count", "auth")),
    ];

    for auth in auths {
        test_dynamic_authlist(3, auth, &[0, 1], true);
    }
}

#[test]
fn dynamic_min_n_of_should_fail_if_not_signed_enough() {
    let auths = [
        rule!(require_n_of(2, "auth")),
        rule!(require_n_of("count", "auth")),
    ];

    for auth in auths {
        test_dynamic_authlist(3, auth, &[0], false);
    }
}

#[test]
fn dynamic_min_n_of_should_fail_if_path_does_not_exist() {
    test_dynamic_authlist(3, rule!(require_n_of(1, "does_not_exist")), &[0, 1], false);
}

#[test]
fn dynamic_all_of_should_allow_me_to_call_method() {
    test_dynamic_authlist(3, rule!(require_all_of("auth")), &[0, 1, 2], true);
}

#[test]
fn dynamic_all_of_should_fail_if_not_signed_enough() {
    test_dynamic_authlist(3, rule!(require_all_of("auth")), &[0, 1], false);
}

#[test]
fn dynamic_all_of_should_fail_if_path_does_not_exist() {
    test_dynamic_authlist(3, rule!(require_all_of("does_not_exist")), &[0, 1], false);
}

#[test]
fn dynamic_any_of_should_allow_me_to_call_method() {
    test_dynamic_authlist(3, rule!(require_any_of("auth")), &[1], true);
}

#[test]
fn dynamic_any_of_should_fail_if_path_does_not_exist() {
    test_dynamic_authlist(3, rule!(require_any_of("does_not_exist")), &[0, 1], false);
}

#[test]
fn chess_should_not_allow_second_player_to_move_if_first_player_didnt_move() {
    // Arrange
    let mut store = TypedInMemorySubstateStore::with_bootstrap();
    let mut test_runner = TestRunner::new(true, &mut store);
    let (pk, _, _) = test_runner.new_account();
    let (other_public_key, _, _) = test_runner.new_account();
    let package = test_runner.extract_and_publish_package("component");
    let non_fungible_address =
        NonFungibleAddress::new(ECDSA_TOKEN, NonFungibleId::from_bytes(pk.to_vec()));
    let other_non_fungible_address = NonFungibleAddress::new(
        ECDSA_TOKEN,
        NonFungibleId::from_bytes(other_public_key.to_vec()),
    );
    let players = [non_fungible_address, other_non_fungible_address];
    let manifest1 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_function(package, "Chess", "create_game", args!(players))
        .build();
    let receipt1 = test_runner.execute_manifest(manifest1, vec![]);
    receipt1.expect_commit_success();
    let component = receipt1
        .expect_commit()
        .entity_changes
        .new_component_addresses[0];

    // Act
    let manifest2 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_method(component, "make_move", args!())
        .build();
    let receipt = test_runner.execute_manifest(manifest2, vec![other_public_key]);

    // Assert
    receipt.expect_specific_failure(is_auth_error);
}

#[test]
fn chess_should_allow_second_player_to_move_after_first_player() {
    // Arrange
    let mut store = TypedInMemorySubstateStore::with_bootstrap();
    let mut test_runner = TestRunner::new(true, &mut store);
    let (public_key, _, _) = test_runner.new_account();
    let (other_public_key, _, _) = test_runner.new_account();
    let package = test_runner.extract_and_publish_package("component");
    let non_fungible_address = NonFungibleAddress::from_public_key(&public_key);
    let other_non_fungible_address = NonFungibleAddress::from_public_key(&other_public_key);
    let players = [non_fungible_address, other_non_fungible_address];
    let manifest1 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_function(package, "Chess", "create_game", args!(players))
        .build();
    let receipt1 = test_runner.execute_manifest(manifest1, vec![]);
    receipt1.expect_commit_success();
    let component = receipt1
        .expect_commit()
        .entity_changes
        .new_component_addresses[0];
    let manifest2 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_method(component, "make_move", args!())
        .build();
    test_runner
        .execute_manifest(manifest2, vec![public_key])
        .expect_commit_success();

    // Act
    let manifest3 = ManifestBuilder::new(&NetworkDefinition::local_simulator())
        .lock_fee(10.into(), SYS_FAUCET_COMPONENT)
        .call_method(component, "make_move", args!())
        .build();
    let receipt = test_runner.execute_manifest(manifest3, vec![other_public_key]);

    // Assert
    receipt.expect_commit_success();
}
