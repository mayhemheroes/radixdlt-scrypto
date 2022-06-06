use sbor::rust::cell::RefCell;
use sbor::rust::collections::BTreeSet;
use sbor::rust::collections::HashMap;
use sbor::rust::rc::Rc;
use sbor::rust::string::ToString;
use sbor::rust::vec::Vec;
use sbor::DecodeError;
use scrypto::buffer::scrypto_decode;
use scrypto::engine::types::*;
use scrypto::prelude::{
    ProofCloneInput, ProofGetAmountInput, ProofGetNonFungibleIdsInput, ProofGetResourceAddressInput,
};
use scrypto::resource::ConsumingProofDropInput;
use scrypto::values::ScryptoValue;

use crate::engine::SystemApi;
use crate::model::ProofError::UnknownMethod;
use crate::model::{
    LockedAmountOrIds, ResourceContainer, ResourceContainerError, ResourceContainerId,
};
use crate::wasm::*;

#[derive(Debug)]
pub struct Proof {
    /// The resource address.
    resource_address: ResourceAddress,
    /// The resource type.
    resource_type: ResourceType,
    /// Whether movement of this proof is restricted.
    restricted: bool,
    /// The total locked amount or non-fungible ids.
    total_locked: LockedAmountOrIds,
    /// The supporting containers.
    evidence: HashMap<ResourceContainerId, (Rc<RefCell<ResourceContainer>>, LockedAmountOrIds)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofError {
    /// Error produced by a resource container.
    ResourceContainerError(ResourceContainerError),
    /// Can't generate zero-amount or empty non-fungible set proofs.
    EmptyProofNotAllowed,
    /// The base proofs are not enough to cover the requested amount or non-fungible ids.
    InsufficientBaseProofs,
    /// Can't apply a non-fungible operation on fungible proofs.
    NonFungibleOperationNotAllowed,
    /// Can't apply a fungible operation on non-fungible proofs.
    FungibleOperationNotAllowed,
    CouldNotCreateProof,
    InvalidRequestData(DecodeError),
    UnknownMethod,
}

impl Proof {
    pub fn new(
        resource_address: ResourceAddress,
        resource_type: ResourceType,
        total_locked: LockedAmountOrIds,
        evidence: HashMap<ResourceContainerId, (Rc<RefCell<ResourceContainer>>, LockedAmountOrIds)>,
    ) -> Result<Proof, ProofError> {
        if total_locked.is_empty() {
            return Err(ProofError::EmptyProofNotAllowed);
        }

        Ok(Self {
            resource_address,
            resource_type,
            restricted: false,
            total_locked,
            evidence,
        })
    }

    /// Computes the locked amount or non-fungible IDs, in total and per resource container.
    pub fn compute_total_locked(
        proofs: &[Proof],
        resource_address: ResourceAddress,
        resource_type: ResourceType,
    ) -> (
        LockedAmountOrIds,
        HashMap<ResourceContainerId, LockedAmountOrIds>,
    ) {
        // filter proofs by resource address and restricted flag
        let proofs: Vec<&Proof> = proofs
            .iter()
            .filter(|p| p.resource_address() == resource_address && !p.is_restricted())
            .collect();

        // calculate the max locked amount (or ids) of each container
        match resource_type {
            ResourceType::Fungible { .. } => {
                let mut max = HashMap::<ResourceContainerId, Decimal>::new();
                for proof in &proofs {
                    for (container_id, (_, locked_amount_or_ids)) in &proof.evidence {
                        let new_amount = locked_amount_or_ids.amount();
                        if let Some(existing) = max.get_mut(&container_id) {
                            *existing = Decimal::max(*existing, new_amount);
                        } else {
                            max.insert(container_id.clone(), new_amount);
                        }
                    }
                }
                let total = max
                    .values()
                    .cloned()
                    .reduce(|a, b| a + b)
                    .unwrap_or_default();
                let per_container = max
                    .into_iter()
                    .map(|(k, v)| (k, LockedAmountOrIds::Amount(v)))
                    .collect();
                (LockedAmountOrIds::Amount(total), per_container)
            }
            ResourceType::NonFungible => {
                let mut max = HashMap::<ResourceContainerId, BTreeSet<NonFungibleId>>::new();
                for proof in &proofs {
                    for (container_id, (_, locked_amount_or_ids)) in &proof.evidence {
                        let new_ids = locked_amount_or_ids.ids().unwrap();
                        if let Some(ids) = max.get_mut(&container_id) {
                            ids.extend(new_ids);
                        } else {
                            max.insert(container_id.clone(), new_ids);
                        }
                    }
                }
                let mut total = BTreeSet::<NonFungibleId>::new();
                for value in max.values() {
                    total.extend(value.clone());
                }
                let per_container = max
                    .into_iter()
                    .map(|(k, v)| (k, LockedAmountOrIds::Ids(v)))
                    .collect();
                (LockedAmountOrIds::Ids(total), per_container)
            }
        }
    }

    /// Creates a composite proof from proofs. This method will generate a max proof.
    pub fn compose(
        proofs: &[Proof],
        resource_address: ResourceAddress,
        resource_type: ResourceType,
    ) -> Result<Proof, ProofError> {
        let (total, _) = Self::compute_total_locked(proofs, resource_address, resource_type);
        match total {
            LockedAmountOrIds::Amount(amount) => {
                Self::compose_by_amount(proofs, amount, resource_address, resource_type)
            }
            LockedAmountOrIds::Ids(ids) => {
                Self::compose_by_ids(proofs, &ids, resource_address, resource_type)
            }
        }
    }

    pub fn compose_by_amount(
        proofs: &[Proof],
        amount: Decimal,
        resource_address: ResourceAddress,
        resource_type: ResourceType,
    ) -> Result<Proof, ProofError> {
        let (total_locked, mut per_container) =
            Self::compute_total_locked(proofs, resource_address, resource_type);

        match total_locked {
            LockedAmountOrIds::Amount(locked_amount) => {
                if amount > locked_amount {
                    return Err(ProofError::InsufficientBaseProofs);
                }

                // Locked the max (or needed) amount from the containers, in the order that the containers were referenced.
                // TODO: observe the performance/feedback of this container selection algorithm and decide next steps
                let mut evidence = HashMap::new();
                let mut remaining = amount.clone();
                'outer: for proof in proofs {
                    for (container_id, (container, _)) in &proof.evidence {
                        if remaining.is_zero() {
                            break 'outer;
                        }

                        if let Some(quota) = per_container.remove(container_id) {
                            let amount = Decimal::min(remaining, quota.amount());
                            container
                                .borrow_mut()
                                .lock_by_amount(amount)
                                .map_err(ProofError::ResourceContainerError)?;
                            remaining -= amount;
                            evidence.insert(
                                container_id.clone(),
                                (container.clone(), LockedAmountOrIds::Amount(amount)),
                            );
                        }
                    }
                }

                Proof::new(
                    resource_address,
                    resource_type,
                    LockedAmountOrIds::Amount(amount),
                    evidence,
                )
            }
            LockedAmountOrIds::Ids(locked_ids) => {
                if amount > locked_ids.len().into() {
                    Err(ProofError::InsufficientBaseProofs)
                } else {
                    let n: usize = amount.to_string().parse().unwrap();
                    let ids: BTreeSet<NonFungibleId> = locked_ids.iter().cloned().take(n).collect();
                    Self::compose_by_ids(proofs, &ids, resource_address, resource_type)
                }
            }
        }
    }

    pub fn compose_by_ids(
        proofs: &[Proof],
        ids: &BTreeSet<NonFungibleId>,
        resource_address: ResourceAddress,
        resource_type: ResourceType,
    ) -> Result<Proof, ProofError> {
        let (total_locked, mut per_container) =
            Self::compute_total_locked(proofs, resource_address, resource_type);

        match total_locked {
            LockedAmountOrIds::Amount(_) => Err(ProofError::NonFungibleOperationNotAllowed),
            LockedAmountOrIds::Ids(locked_ids) => {
                if !locked_ids.is_superset(ids) {
                    return Err(ProofError::InsufficientBaseProofs);
                }

                // Locked the max (or needed) ids from the containers, in the order that the containers were referenced.
                // TODO: observe the performance/feedback of this container selection algorithm and decide next steps
                let mut evidence = HashMap::new();
                let mut remaining = ids.clone();
                'outer: for proof in proofs {
                    for (container_id, (container, _)) in &proof.evidence {
                        if remaining.is_empty() {
                            break 'outer;
                        }

                        if let Some(quota) = per_container.remove(container_id) {
                            let ids = remaining
                                .intersection(&quota.ids().unwrap())
                                .cloned()
                                .collect();
                            container
                                .borrow_mut()
                                .lock_by_ids(&ids)
                                .map_err(ProofError::ResourceContainerError)?;
                            for id in &ids {
                                remaining.remove(id);
                            }
                            evidence.insert(
                                container_id.clone(),
                                (container.clone(), LockedAmountOrIds::Ids(ids)),
                            );
                        }
                    }
                }

                Proof::new(
                    resource_address,
                    resource_type,
                    LockedAmountOrIds::Ids(ids.clone()),
                    evidence,
                )
            }
        }
    }

    /// Makes a clone of this proof.
    ///
    /// Note that cloning a proof will update the ref count of the locked
    /// resources in the source containers.
    pub fn clone(&self) -> Self {
        for (_, (container, locked_amount_or_ids)) in &self.evidence {
            match locked_amount_or_ids {
                LockedAmountOrIds::Amount(amount) => {
                    container
                        .borrow_mut()
                        .lock_by_amount(*amount)
                        .expect("Cloning should always succeed");
                }
                LockedAmountOrIds::Ids(ids) => {
                    container
                        .borrow_mut()
                        .lock_by_ids(ids)
                        .expect("Cloning should always succeed");
                }
            }
        }
        Self {
            resource_address: self.resource_address.clone(),
            resource_type: self.resource_type.clone(),
            restricted: self.restricted,
            total_locked: self.total_locked.clone(),
            evidence: self.evidence.clone(),
        }
    }

    pub fn drop(self) {
        for (_, (container, locked_amount_or_ids)) in self.evidence {
            container.borrow_mut().unlock(locked_amount_or_ids);
        }
    }

    // FIXME: this is a hack for now until we can get snode_state into process
    // FIXME: and be able to determine which snode the proof is going into
    pub fn change_to_unrestricted(&mut self) {
        self.restricted = false;
    }

    pub fn change_to_restricted(&mut self) {
        self.restricted = true;
    }

    pub fn resource_address(&self) -> ResourceAddress {
        self.resource_address
    }

    pub fn total_amount(&self) -> Decimal {
        self.total_locked.amount()
    }

    pub fn total_ids(&self) -> Result<BTreeSet<NonFungibleId>, ProofError> {
        self.total_locked
            .ids()
            .map_err(|_| ProofError::NonFungibleOperationNotAllowed)
    }

    pub fn is_restricted(&self) -> bool {
        self.restricted
    }

    pub fn main<S: SystemApi<W, I>, W: WasmEngine<I>, I: WasmInstance>(
        &mut self,
        method_name: &str,
        arg: ScryptoValue,
        system_api: &mut S,
    ) -> Result<ScryptoValue, ProofError> {
        match method_name {
            "amount" => {
                let _: ProofGetAmountInput =
                    scrypto_decode(&arg.raw).map_err(|e| ProofError::InvalidRequestData(e))?;
                Ok(ScryptoValue::from_trusted(&self.total_amount()))
            }
            "non_fungible_ids" => {
                let _: ProofGetNonFungibleIdsInput =
                    scrypto_decode(&arg.raw).map_err(|e| ProofError::InvalidRequestData(e))?;
                let ids = self.total_ids()?;
                return Ok(ScryptoValue::from_trusted(&ids));
            }
            "resource_address" => {
                let _: ProofGetResourceAddressInput =
                    scrypto_decode(&arg.raw).map_err(|e| ProofError::InvalidRequestData(e))?;
                Ok(ScryptoValue::from_trusted(&self.resource_address()))
            }
            "clone" => {
                let _: ProofCloneInput =
                    scrypto_decode(&arg.raw).map_err(|e| ProofError::InvalidRequestData(e))?;
                let cloned_proof = self.clone();
                let proof_id = system_api
                    .create_proof(cloned_proof)
                    .map_err(|_| ProofError::CouldNotCreateProof)?;
                Ok(ScryptoValue::from_trusted(&scrypto::resource::Proof(
                    proof_id,
                )))
            }
            _ => Err(UnknownMethod),
        }
    }

    pub fn main_consume(
        self,
        method_name: &str,
        arg: ScryptoValue,
    ) -> Result<ScryptoValue, ProofError> {
        match method_name {
            "drop" => {
                let _: ConsumingProofDropInput =
                    scrypto_decode(&arg.raw).map_err(|e| ProofError::InvalidRequestData(e))?;
                self.drop();
                Ok(ScryptoValue::from_trusted(&()))
            }
            _ => Err(UnknownMethod),
        }
    }
}
