use clap::Parser;
use colored::*;
use rand::Rng;
use scrypto::prelude::*;
use transaction::signing::EcdsaPrivateKey;

use crate::resim::*;

/// Generate a key pair
#[derive(Parser, Debug)]
pub struct GenerateKeyPair {}

impl GenerateKeyPair {
    pub fn run<O: std::io::Write>(&self, out: &mut O) -> Result<(), Error> {
        let secret = rand::thread_rng().gen::<[u8; 32]>();
        let private_key = EcdsaPrivateKey::from_bytes(&secret).unwrap();
        let public_key = private_key.public_key();
        writeln!(out, "Public key: {}", public_key.to_string().green()).map_err(Error::IOError)?;
        writeln!(
            out,
            "Private key: {}",
            hex::encode(private_key.to_bytes()).green()
        )
        .map_err(Error::IOError)?;
        Ok(())
    }
}
