use core::fmt::Debug;

use crate::engine::*;
use crate::fee::FeeReserve;
use crate::types::*;
use crate::wasm::*;

/// A collection of blueprints, compiled and published as a single unit.
#[derive(Clone, TypeId, Encode, Decode, PartialEq, Eq)]
pub struct ValidatedPackage {
    code: Vec<u8>,
    blueprint_abis: HashMap<String, BlueprintAbi>,
}

#[derive(Debug)]
pub enum PackageError {
    RuntimeError(Box<RuntimeError>),
    InvalidRequestData(DecodeError),
    InvalidWasm(PrepareError),
    BlueprintNotFound,
    MethodNotFound(String),
}

impl ValidatedPackage {
    pub fn new(package: scrypto::prelude::Package) -> Result<Self, PrepareError> {
        WasmValidator::default().validate(&package.code, &package.blueprints)?;

        Ok(Self {
            code: package.code,
            blueprint_abis: package.blueprints,
        })
    }

    pub fn code(&self) -> &[u8] {
        &self.code
    }

    pub fn blueprint_abi(&self, blueprint_name: &str) -> Option<&BlueprintAbi> {
        self.blueprint_abis.get(blueprint_name)
    }

    pub fn static_main<'s, Y, W, I, R>(
        package_fn: PackageFnIdentifier,
        call_data: ScryptoValue,
        system_api: &mut Y,
    ) -> Result<ScryptoValue, PackageError>
    where
        Y: SystemApi<'s, W, I, R>,
        W: WasmEngine<I>,
        I: WasmInstance,
        R: FeeReserve,
    {
        match package_fn {
            PackageFnIdentifier::Publish => {
                let input: PackagePublishInput = scrypto_decode(&call_data.raw)
                    .map_err(|e| PackageError::InvalidRequestData(e))?;
                let package =
                    ValidatedPackage::new(input.package).map_err(PackageError::InvalidWasm)?;
                let node_id = system_api
                    .node_create(HeapRENode::Package(package))
                    .map_err(|e| PackageError::RuntimeError(Box::new(e)))?;
                system_api
                    .node_globalize(node_id)
                    .map_err(|e| PackageError::RuntimeError(Box::new(e)))?;
                let package_address: PackageAddress = node_id.into();
                Ok(ScryptoValue::from_typed(&package_address))
            }
        }
    }
}

impl Debug for ValidatedPackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatedPackage")
            .field("code_len", &self.code.len())
            .field("blueprint_abis", &self.blueprint_abis)
            .finish()
    }
}
