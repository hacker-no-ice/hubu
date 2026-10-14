pub mod app;
pub mod budget;
pub mod ledger;
pub mod persistence;
pub mod policy;
pub mod registration;
pub mod spend;
pub mod spending_target;
pub mod storage;
pub mod telemetry;
pub mod user;

#[cfg(test)]
pub(crate) mod test_money {
    use hubu_common::money::Amount;

    /// A whole-cent amount at accounting scale, for test fixtures.
    pub(crate) fn cents(cents: i64) -> Amount {
        Amount::from_legacy_cents(cents).expect("test amount fits the accounting range")
    }
}
