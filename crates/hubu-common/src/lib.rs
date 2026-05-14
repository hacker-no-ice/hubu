use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type Amount = u128;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PaymentIntent {
    pub id: Uuid,
    pub payer: String,
    pub payee: String,
    pub amount: Amount,
    pub asset: String,
    pub memo: Option<String>,
}

impl PaymentIntent {
    pub fn new(
        payer: impl Into<String>,
        payee: impl Into<String>,
        amount: Amount,
        asset: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            payer: payer.into(),
            payee: payee.into(),
            amount,
            asset: asset.into(),
            memo: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: Uuid,
    pub payment_intent_id: Uuid,
    pub amount: Amount,
    pub asset: String,
    pub status: LedgerEntryStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LedgerEntryStatus {
    Pending,
    Settled,
    Failed,
}
