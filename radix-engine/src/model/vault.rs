use crate::engine::{HeapRENode, RuntimeError, SystemApi};
use crate::fee::{FeeReserve, FeeReserveError};
use crate::model::{
    Bucket, Proof, ProofError, ResourceContainer, ResourceContainerError, ResourceContainerId,
};
use crate::types::*;
use crate::wasm::*;

#[derive(Debug)]
pub enum VaultError {
    RuntimeError(Box<RuntimeError>),
    InvalidRequestData(DecodeError),
    ResourceContainerError(ResourceContainerError),
    CouldNotCreateBucket,
    CouldNotTakeBucket,
    ProofError(ProofError),
    CouldNotCreateProof,
    LockFeeNotRadixToken,
    LockFeeInsufficientBalance,
    LockFeeRepayFailure(FeeReserveError),
}

/// A persistent resource container.
#[derive(Debug, Clone, TypeId, Encode, Decode, PartialEq, Eq)]
pub struct Vault {
    container: Rc<RefCell<ResourceContainer>>,
}

impl Vault {
    pub fn new(container: ResourceContainer) -> Self {
        Self {
            container: Rc::new(RefCell::new(container)),
        }
    }

    pub fn put(&mut self, other: Bucket) -> Result<(), ResourceContainerError> {
        self.borrow_container_mut().put(other.into_container()?)
    }

    fn take(&mut self, amount: Decimal) -> Result<ResourceContainer, VaultError> {
        let container = self
            .borrow_container_mut()
            .take_by_amount(amount)
            .map_err(VaultError::ResourceContainerError)?;
        Ok(container)
    }

    fn take_non_fungibles(
        &mut self,
        ids: &BTreeSet<NonFungibleId>,
    ) -> Result<ResourceContainer, VaultError> {
        let container = self
            .borrow_container_mut()
            .take_by_ids(ids)
            .map_err(VaultError::ResourceContainerError)?;
        Ok(container)
    }

    pub fn create_proof(&mut self, container_id: ResourceContainerId) -> Result<Proof, ProofError> {
        match self.resource_type() {
            ResourceType::Fungible { .. } => {
                self.create_proof_by_amount(self.total_amount(), container_id)
            }
            ResourceType::NonFungible => self.create_proof_by_ids(
                &self
                    .total_ids()
                    .expect("Failed to list non-fungible IDs of non-fungible vault"),
                container_id,
            ),
        }
    }

    pub fn create_proof_by_amount(
        &mut self,
        amount: Decimal,
        container_id: ResourceContainerId,
    ) -> Result<Proof, ProofError> {
        // lock the specified amount
        let locked_amount_or_ids = self
            .borrow_container_mut()
            .lock_by_amount(amount)
            .map_err(ProofError::ResourceContainerError)?;

        // produce proof
        let mut evidence = HashMap::new();
        evidence.insert(
            container_id,
            (self.container.clone(), locked_amount_or_ids.clone()),
        );
        Proof::new(
            self.resource_address(),
            self.resource_type(),
            locked_amount_or_ids,
            evidence,
        )
    }

    pub fn create_proof_by_ids(
        &mut self,
        ids: &BTreeSet<NonFungibleId>,
        container_id: ResourceContainerId,
    ) -> Result<Proof, ProofError> {
        // lock the specified id set
        let locked_amount_or_ids = self
            .borrow_container_mut()
            .lock_by_ids(ids)
            .map_err(ProofError::ResourceContainerError)?;

        // produce proof
        let mut evidence = HashMap::new();
        evidence.insert(
            container_id,
            (self.container.clone(), locked_amount_or_ids.clone()),
        );
        Proof::new(
            self.resource_address(),
            self.resource_type(),
            locked_amount_or_ids,
            evidence,
        )
    }

    pub fn resource_address(&self) -> ResourceAddress {
        self.borrow_container().resource_address()
    }

    pub fn resource_type(&self) -> ResourceType {
        self.borrow_container().resource_type()
    }

    pub fn total_amount(&self) -> Decimal {
        self.borrow_container().total_amount()
    }

    pub fn total_ids(&self) -> Result<BTreeSet<NonFungibleId>, ResourceContainerError> {
        self.borrow_container().total_ids()
    }

    pub fn is_locked(&self) -> bool {
        self.borrow_container().is_locked()
    }

    pub fn is_empty(&self) -> bool {
        self.borrow_container().is_empty()
    }

    fn borrow_container(&self) -> Ref<ResourceContainer> {
        self.container.borrow()
    }

    fn borrow_container_mut(&mut self) -> RefMut<ResourceContainer> {
        self.container.borrow_mut()
    }

    pub fn main<'s, Y, W, I, R>(
        vault_id: VaultId,
        vault_fn: VaultFnIdentifier,
        args: ScryptoValue,
        system_api: &mut Y,
    ) -> Result<ScryptoValue, VaultError>
    where
        Y: SystemApi<'s, W, I, R>,
        W: WasmEngine<I>,
        I: WasmInstance,
        R: FeeReserve,
    {
        let substate_id = SubstateId::Vault(vault_id.clone());
        let mut ref_mut = system_api
            .substate_borrow_mut(&substate_id)
            .map_err(|e| VaultError::RuntimeError(Box::new(e)))?;
        let vault = ref_mut.vault();

        let rtn = match vault_fn {
            VaultFnIdentifier::Put => {
                let input: VaultPutInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let bucket = system_api
                    .node_drop(&RENodeId::Bucket(input.bucket.0))
                    .map_err(|e| VaultError::RuntimeError(Box::new(e)))?
                    .into();
                vault
                    .put(bucket)
                    .map_err(VaultError::ResourceContainerError)?;
                Ok(ScryptoValue::from_typed(&()))
            }
            VaultFnIdentifier::Take => {
                let input: VaultTakeInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let container = vault.take(input.amount)?;
                let bucket_id = system_api
                    .node_create(HeapRENode::Bucket(Bucket::new(container)))
                    .map_err(|e| VaultError::RuntimeError(Box::new(e)))?
                    .into();
                Ok(ScryptoValue::from_typed(&scrypto::resource::Bucket(
                    bucket_id,
                )))
            }
            VaultFnIdentifier::LockFee | VaultFnIdentifier::LockContingentFee => {
                let input: VaultLockFeeInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;

                // Check resource and take amount
                if vault.resource_address() != RADIX_TOKEN {
                    return Err(VaultError::LockFeeNotRadixToken);
                }

                // Take fee from the vault
                let fee = vault
                    .take(input.amount)
                    .map_err(|_| VaultError::LockFeeInsufficientBalance)?;

                // Refill fee reserve
                let changes = system_api
                    .lock_fee(
                        vault_id,
                        fee,
                        matches!(vault_fn, VaultFnIdentifier::LockContingentFee),
                    )
                    .map_err(|e| VaultError::RuntimeError(Box::new(e)))?;

                // Return changes
                vault
                    .borrow_container_mut()
                    .put(changes)
                    .expect("Failed to return fee changes to a locking-fee vault");

                Ok(ScryptoValue::from_typed(&()))
            }
            VaultFnIdentifier::TakeNonFungibles => {
                let input: VaultTakeNonFungiblesInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let container = vault.take_non_fungibles(&input.non_fungible_ids)?;
                let bucket_id = system_api
                    .node_create(HeapRENode::Bucket(Bucket::new(container)))
                    .map_err(|e| VaultError::RuntimeError(Box::new(e)))?
                    .into();
                Ok(ScryptoValue::from_typed(&scrypto::resource::Bucket(
                    bucket_id,
                )))
            }
            VaultFnIdentifier::GetAmount => {
                let _: VaultGetAmountInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let amount = vault.total_amount();
                Ok(ScryptoValue::from_typed(&amount))
            }
            VaultFnIdentifier::GetResourceAddress => {
                let _: VaultGetResourceAddressInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let resource_address = vault.resource_address();
                Ok(ScryptoValue::from_typed(&resource_address))
            }
            VaultFnIdentifier::GetNonFungibleIds => {
                let _: VaultGetNonFungibleIdsInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let ids = vault
                    .total_ids()
                    .map_err(VaultError::ResourceContainerError)?;
                Ok(ScryptoValue::from_typed(&ids))
            }
            VaultFnIdentifier::CreateProof => {
                let _: VaultCreateProofInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let proof = vault
                    .create_proof(ResourceContainerId::Vault(vault_id))
                    .map_err(VaultError::ProofError)?;
                let proof_id = system_api
                    .node_create(HeapRENode::Proof(proof))
                    .map_err(|e| VaultError::RuntimeError(Box::new(e)))?
                    .into();
                Ok(ScryptoValue::from_typed(&scrypto::resource::Proof(
                    proof_id,
                )))
            }
            VaultFnIdentifier::CreateProofByAmount => {
                let input: VaultCreateProofByAmountInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let proof = vault
                    .create_proof_by_amount(input.amount, ResourceContainerId::Vault(vault_id))
                    .map_err(VaultError::ProofError)?;
                let proof_id = system_api
                    .node_create(HeapRENode::Proof(proof))
                    .map_err(|e| VaultError::RuntimeError(Box::new(e)))?
                    .into();
                Ok(ScryptoValue::from_typed(&scrypto::resource::Proof(
                    proof_id,
                )))
            }
            VaultFnIdentifier::CreateProofByIds => {
                let input: VaultCreateProofByIdsInput =
                    scrypto_decode(&args.raw).map_err(|e| VaultError::InvalidRequestData(e))?;
                let proof = vault
                    .create_proof_by_ids(&input.ids, ResourceContainerId::Vault(vault_id))
                    .map_err(VaultError::ProofError)?;
                let proof_id = system_api
                    .node_create(HeapRENode::Proof(proof))
                    .map_err(|e| VaultError::RuntimeError(Box::new(e)))?
                    .into();
                Ok(ScryptoValue::from_typed(&scrypto::resource::Proof(
                    proof_id,
                )))
            }
        }?;

        system_api
            .substate_return_mut(ref_mut)
            .map_err(|e| VaultError::RuntimeError(Box::new(e)))?;

        Ok(rtn)
    }
}
