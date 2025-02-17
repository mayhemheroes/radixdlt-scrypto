use clap::Parser;
use regex::{Captures, Regex};
use scrypto::core::NetworkDefinition;
use std::env;
use std::path::PathBuf;

use crate::resim::*;

/// Compiles, signs and runs a transaction manifest
#[derive(Parser, Debug)]
pub struct Run {
    /// The path to a transaction manifest file
    path: PathBuf,

    /// The private keys used for signing, separated by comma
    #[clap(short, long)]
    signing_keys: Option<String>,

    /// Turn on tracing
    #[clap(short, long)]
    trace: bool,
}

impl Run {
    pub fn pre_process_manifest(manifest: &str) -> String {
        let re = Regex::new(r"\$\{(.+?)\}").unwrap();
        re.replace_all(manifest, |caps: &Captures| {
            env::var(&caps[1].trim()).unwrap_or_default()
        })
        .into()
    }

    pub fn run<O: std::io::Write>(&self, out: &mut O) -> Result<(), Error> {
        let manifest = std::fs::read_to_string(&self.path).map_err(Error::IOError)?;
        let pre_processed_manifest = Self::pre_process_manifest(&manifest);
        let compiled_manifest = transaction::manifest::compile(
            &pre_processed_manifest,
            &NetworkDefinition::local_simulator(),
        )
        .map_err(Error::CompileError)?;
        handle_manifest(
            compiled_manifest,
            &self.signing_keys,
            &None,
            false,
            self.trace,
            true,
            out,
        )
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pre_process_manifest() {
        temp_env::with_vars(
            vec![
                (
                    "faucet",
                    Some("system_sim1qsqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqpql4sktx"),
                ),
                (
                    "xrd",
                    Some("resource_sim1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzqu57yag"),
                ),
            ],
            || {
                let manifest = r#"CALL_METHOD ComponentAddress("${  faucet  }") "free_xrd";\nTAKE_FROM_WORKTOP ResourceAddress("${xrd}") Bucket("bucket1");\n"#;
                let after = r#"CALL_METHOD ComponentAddress("system_sim1qsqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqpql4sktx") "free_xrd";\nTAKE_FROM_WORKTOP ResourceAddress("resource_sim1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzqu57yag") Bucket("bucket1");\n"#;
                assert_eq!(Run::pre_process_manifest(manifest), after);
            },
        );
    }
}
