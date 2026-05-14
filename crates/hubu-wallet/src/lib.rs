use hubu_common::PaymentIntent;
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct Wallet {
    address: String,
}

impl Wallet {
    pub fn from_address(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn sign_payment_intent(&self, intent: &PaymentIntent) -> Result<Signature, WalletError> {
        if self.address.is_empty() {
            return Err(WalletError::MissingAddress);
        }

        Ok(Signature {
            signer: self.address.clone(),
            payload_id: intent.id.to_string(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    pub signer: String,
    pub payload_id: String,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum WalletError {
    #[error("wallet address is missing")]
    MissingAddress,
}
