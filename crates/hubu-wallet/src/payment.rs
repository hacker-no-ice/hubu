use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use hubu_common::ids::{
    AgentAccountId, AgentId, LedgerTransactionId, PaymentId, SpendAuthTokenId, UserId,
};
use hubu_common::money::Currency;

use crate::ledger::{
    LedgerAccount, LedgerAccountKind, LedgerDirection, LedgerEntryDraft, LedgerError, SqliteLedger,
};
use crate::rail::{PaymentRail, PaymentRailError};

#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("payment amount must be positive")]
    NonPositiveAmount,
    #[error("idempotency key cannot be empty")]
    EmptyIdempotencyKey,
    #[error("idempotency key was reused with a different payment request")]
    IdempotencyConflict,
    #[error("spend authorization rejected payment request: {reason}")]
    AuthorizationRejected { reason: String },
    #[error("payment rail error")]
    Rail {
        #[from]
        source: PaymentRailError,
    },
    #[error("ledger error")]
    Ledger {
        #[from]
        source: LedgerError,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PaymentStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PaymentRailKind {
    FiatMock,
    StablecoinMock,
}

impl AsRef<str> for PaymentRailKind {
    fn as_ref(&self) -> &str {
        match self {
            PaymentRailKind::FiatMock => "fiat_mock",
            PaymentRailKind::StablecoinMock => "stablecoin_mock",
        }
    }
}

impl fmt::Display for PaymentRailKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PaymentDestination {
    FiatAccount { account_ref: String },
    StablecoinWallet { chain: String, address: String },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PaymentRequest {
    pub idempotency_key: String,
    pub spend_auth_token_id: SpendAuthTokenId,
    pub owner_user_id: UserId,
    pub agent_id: AgentId,
    pub agent_account_id: AgentAccountId,
    pub amount_cents: i64,
    pub currency: Currency,
    pub merchant: Option<String>,
    pub task_id: Option<String>,
    pub rail: PaymentRailKind,
    pub destination: PaymentDestination,
    pub memo: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PaymentResponse {
    pub payment_id: PaymentId,
    pub owner_user_id: UserId,
    pub agent_account_id: AgentAccountId,
    pub status: PaymentStatus,
    pub amount_cents: i64,
    pub currency: Currency,
    pub ledger_transaction_id: Option<LedgerTransactionId>,
    pub rail_reference: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ValidatedSpendAuthorization {
    pub spend_auth_token_id: SpendAuthTokenId,
    pub owner_user_id: UserId,
}

pub trait SpendAuthorizationValidator {
    fn validate_payment_request(
        &self,
        request: &PaymentRequest,
    ) -> Result<ValidatedSpendAuthorization, PaymentError>;

    fn mark_token_used(
        &mut self,
        token_id: &SpendAuthTokenId,
        payment_id: &PaymentId,
    ) -> Result<(), PaymentError>;
}

#[derive(Debug, Clone)]
struct StoredPayment {
    request: PaymentRequest,
    response: PaymentResponse,
}

#[derive(Debug, Clone)]
struct PaymentLedgerAccounts {
    wallet_cash: LedgerAccount,
    agent_spend_expense: LedgerAccount,
}

pub struct PaymentManager<R, A> {
    rail: R,
    authorizer: A,
    ledger: SqliteLedger,
    ledger_accounts_by_user: HashMap<UserId, PaymentLedgerAccounts>,
    payments_by_idempotency_key: HashMap<String, StoredPayment>,
}

impl<R, A> PaymentManager<R, A>
where
    R: PaymentRail,
    A: SpendAuthorizationValidator,
{
    pub fn new(
        owner_user_id: UserId,
        rail: R,
        authorizer: A,
        ledger: SqliteLedger,
    ) -> Result<Self, PaymentError> {
        let wallet_cash = ledger.create_account(
            owner_user_id.clone(),
            "Hubu wallet cash",
            LedgerAccountKind::UserWalletCash,
            Currency::Usd,
        )?;
        let agent_spend_expense = ledger.create_account(
            owner_user_id.clone(),
            "Agent spend expense",
            LedgerAccountKind::AgentSpendExpense,
            Currency::Usd,
        )?;
        let mut ledger_accounts_by_user = HashMap::new();
        ledger_accounts_by_user.insert(
            owner_user_id,
            PaymentLedgerAccounts {
                wallet_cash,
                agent_spend_expense,
            },
        );

        Ok(Self {
            rail,
            authorizer,
            ledger,
            ledger_accounts_by_user,
            payments_by_idempotency_key: HashMap::new(),
        })
    }

    pub fn submit_payment(
        &mut self,
        request: PaymentRequest,
    ) -> Result<PaymentResponse, PaymentError> {
        self.validate_request_shape(&request)?;

        if let Some(stored) = self
            .payments_by_idempotency_key
            .get(&request.idempotency_key)
        {
            if stored.request != request {
                return Err(PaymentError::IdempotencyConflict);
            }

            return Ok(stored.response.clone());
        }

        let payment_id = PaymentId::new();
        let created_at = Utc::now();

        self.authorizer.validate_payment_request(&request)?;
        let rail_result = self.rail.execute(&request)?;

        let response = if rail_result.status == PaymentStatus::Succeeded {
            let ledger_accounts = ledger_accounts_for_user(
                &self.ledger,
                &mut self.ledger_accounts_by_user,
                &request.owner_user_id,
            )?;
            let ledger_transaction = self.ledger.record_transaction(
                request.owner_user_id.clone(),
                Some(payment_id.to_string()),
                format!("payment {} via {}", payment_id, request.rail.as_ref()),
                vec![
                    LedgerEntryDraft {
                        owner_user_id: request.owner_user_id.clone(),
                        account_id: ledger_accounts.agent_spend_expense.id.clone(),
                        direction: LedgerDirection::Debit,
                        amount_cents: request.amount_cents,
                        currency: request.currency,
                    },
                    LedgerEntryDraft {
                        owner_user_id: request.owner_user_id.clone(),
                        account_id: ledger_accounts.wallet_cash.id.clone(),
                        direction: LedgerDirection::Credit,
                        amount_cents: request.amount_cents,
                        currency: request.currency,
                    },
                ],
            )?;

            self.authorizer
                .mark_token_used(&request.spend_auth_token_id, &payment_id)?;

            PaymentResponse {
                payment_id,
                owner_user_id: request.owner_user_id.clone(),
                agent_account_id: request.agent_account_id.clone(),
                status: PaymentStatus::Succeeded,
                amount_cents: request.amount_cents,
                currency: request.currency,
                ledger_transaction_id: Some(ledger_transaction.id),
                rail_reference: rail_result.rail_reference,
                failure_reason: None,
                created_at,
            }
        } else {
            PaymentResponse {
                payment_id,
                owner_user_id: request.owner_user_id.clone(),
                agent_account_id: request.agent_account_id.clone(),
                status: PaymentStatus::Failed,
                amount_cents: request.amount_cents,
                currency: request.currency,
                ledger_transaction_id: None,
                rail_reference: rail_result.rail_reference,
                failure_reason: rail_result.failure_reason,
                created_at,
            }
        };

        self.payments_by_idempotency_key.insert(
            request.idempotency_key.clone(),
            StoredPayment {
                request,
                response: response.clone(),
            },
        );

        Ok(response)
    }

    pub fn remember_payment_attempt(
        &mut self,
        request: PaymentRequest,
        response: PaymentResponse,
    ) -> Result<(), PaymentError> {
        self.validate_request_shape(&request)?;

        if let Some(stored) = self
            .payments_by_idempotency_key
            .get(&request.idempotency_key)
        {
            if stored.request != request {
                return Err(PaymentError::IdempotencyConflict);
            }

            return Ok(());
        }

        self.payments_by_idempotency_key.insert(
            request.idempotency_key.clone(),
            StoredPayment { request, response },
        );
        Ok(())
    }

    pub fn ledger(&self) -> &SqliteLedger {
        &self.ledger
    }

    fn validate_request_shape(&self, request: &PaymentRequest) -> Result<(), PaymentError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(PaymentError::EmptyIdempotencyKey);
        }

        if request.amount_cents <= 0 {
            return Err(PaymentError::NonPositiveAmount);
        }

        Ok(())
    }
}

fn ledger_accounts_for_user(
    ledger: &SqliteLedger,
    ledger_accounts_by_user: &mut HashMap<UserId, PaymentLedgerAccounts>,
    owner_user_id: &UserId,
) -> Result<PaymentLedgerAccounts, PaymentError> {
    if let Some(accounts) = ledger_accounts_by_user.get(owner_user_id) {
        return Ok(accounts.clone());
    }

    let wallet_cash = ledger.create_account(
        owner_user_id.clone(),
        "Hubu wallet cash",
        LedgerAccountKind::UserWalletCash,
        Currency::Usd,
    )?;
    let agent_spend_expense = ledger.create_account(
        owner_user_id.clone(),
        "Agent spend expense",
        LedgerAccountKind::AgentSpendExpense,
        Currency::Usd,
    )?;
    let accounts = PaymentLedgerAccounts {
        wallet_cash,
        agent_spend_expense,
    };
    ledger_accounts_by_user.insert(owner_user_id.clone(), accounts.clone());

    Ok(accounts)
}

#[cfg(test)]
mod tests {
    use hubu_common::ids::SpendAuthTokenId;

    use super::*;
    use crate::rail::MockPaymentRail;

    #[derive(Debug, Clone)]
    struct TestAuthorizer {
        token_id: SpendAuthTokenId,
        owner_user_id: UserId,
        agent_id: AgentId,
        agent_account_id: AgentAccountId,
        amount_cents: i64,
        currency: Currency,
        merchant: Option<String>,
        task_id: Option<String>,
        used_payment_id: Option<PaymentId>,
    }

    impl TestAuthorizer {
        fn for_request(request: &PaymentRequest) -> Self {
            Self {
                token_id: request.spend_auth_token_id.clone(),
                owner_user_id: request.owner_user_id.clone(),
                agent_id: request.agent_id.clone(),
                agent_account_id: request.agent_account_id.clone(),
                amount_cents: request.amount_cents,
                currency: request.currency,
                merchant: request.merchant.clone(),
                task_id: request.task_id.clone(),
                used_payment_id: None,
            }
        }
    }

    impl SpendAuthorizationValidator for TestAuthorizer {
        fn validate_payment_request(
            &self,
            request: &PaymentRequest,
        ) -> Result<ValidatedSpendAuthorization, PaymentError> {
            let matches_authorized_spend = request.spend_auth_token_id == self.token_id
                && request.owner_user_id == self.owner_user_id
                && request.agent_id == self.agent_id
                && request.agent_account_id == self.agent_account_id
                && request.amount_cents == self.amount_cents
                && request.currency == self.currency
                && request.merchant == self.merchant
                && request.task_id == self.task_id;

            if !matches_authorized_spend {
                return Err(PaymentError::AuthorizationRejected {
                    reason: "payment request does not match authorized spend".to_string(),
                });
            }

            Ok(ValidatedSpendAuthorization {
                spend_auth_token_id: request.spend_auth_token_id.clone(),
                owner_user_id: request.owner_user_id.clone(),
            })
        }

        fn mark_token_used(
            &mut self,
            token_id: &SpendAuthTokenId,
            payment_id: &PaymentId,
        ) -> Result<(), PaymentError> {
            if token_id != &self.token_id {
                return Err(PaymentError::AuthorizationRejected {
                    reason: "unknown token".to_string(),
                });
            }

            self.used_payment_id = Some(payment_id.clone());
            Ok(())
        }
    }

    fn payment_request(rail: PaymentRailKind) -> PaymentRequest {
        let destination = match rail {
            PaymentRailKind::FiatMock => PaymentDestination::FiatAccount {
                account_ref: "acct_123".to_string(),
            },
            PaymentRailKind::StablecoinMock => PaymentDestination::StablecoinWallet {
                chain: "base-sepolia".to_string(),
                address: "0x0000000000000000000000000000000000000001".to_string(),
            },
        };

        PaymentRequest {
            idempotency_key: format!("idem_{}", rail.as_ref()),
            spend_auth_token_id: SpendAuthTokenId::new(),
            owner_user_id: test_user_id(),
            agent_id: AgentId::new(),
            agent_account_id: AgentAccountId::new(),
            amount_cents: 2_500,
            currency: Currency::Usd,
            merchant: Some("Acme Cafe".to_string()),
            task_id: Some("task_123".to_string()),
            rail,
            destination,
            memo: Some("lunch".to_string()),
        }
    }

    fn test_user_id() -> UserId {
        "00000000-0000-4000-8000-000000000123".parse().unwrap()
    }

    fn payment_manager(
        request: &PaymentRequest,
    ) -> PaymentManager<MockPaymentRail, TestAuthorizer> {
        PaymentManager::new(
            request.owner_user_id.clone(),
            MockPaymentRail,
            TestAuthorizer::for_request(request),
            SqliteLedger::in_memory().expect("ledger should initialize"),
        )
        .expect("payment manager should initialize")
    }

    #[test]
    fn fiat_payment_success_validates_auth_and_records_balanced_ledger_transaction() {
        let request = payment_request(PaymentRailKind::FiatMock);
        let mut manager = payment_manager(&request);

        let response = manager
            .submit_payment(request)
            .expect("payment should succeed");

        assert_eq!(response.status, PaymentStatus::Succeeded);
        assert_eq!(response.owner_user_id, test_user_id());
        assert_eq!(response.amount_cents, 2_500);
        assert_eq!(response.currency, Currency::Usd);
        assert!(response
            .rail_reference
            .as_deref()
            .unwrap()
            .starts_with("fiat_mock"));
        assert_eq!(
            manager.authorizer.used_payment_id,
            Some(response.payment_id.clone())
        );

        let ledger_transaction_id = response
            .ledger_transaction_id
            .expect("successful payment should record ledger transaction");
        let entries = manager
            .ledger()
            .entries_for_transaction(&ledger_transaction_id)
            .expect("ledger entries should be readable");

        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|entry| entry.owner_user_id == test_user_id()));
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.direction == LedgerDirection::Debit)
                .map(|entry| entry.amount_cents)
                .sum::<i64>(),
            2_500
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.direction == LedgerDirection::Credit)
                .map(|entry| entry.amount_cents)
                .sum::<i64>(),
            2_500
        );
    }

    #[test]
    fn stablecoin_payment_uses_same_orchestration_contract() {
        let request = payment_request(PaymentRailKind::StablecoinMock);
        let mut manager = payment_manager(&request);

        let response = manager
            .submit_payment(request)
            .expect("stablecoin payment should succeed");

        assert_eq!(response.status, PaymentStatus::Succeeded);
        assert!(response
            .rail_reference
            .as_deref()
            .unwrap()
            .starts_with("stablecoin_mock"));
        assert!(response.ledger_transaction_id.is_some());
    }

    #[test]
    fn idempotency_key_returns_original_response_for_same_request() {
        let request = payment_request(PaymentRailKind::FiatMock);
        let mut manager = payment_manager(&request);

        let first = manager
            .submit_payment(request.clone())
            .expect("first payment should succeed");
        let second = manager
            .submit_payment(request)
            .expect("second payment should be idempotent");

        assert_eq!(first.payment_id, second.payment_id);
        assert_eq!(manager.payments_by_idempotency_key.len(), 1);
    }

    #[test]
    fn remembered_payment_attempt_returns_original_response_without_revalidating() {
        let request = payment_request(PaymentRailKind::FiatMock);
        let mut manager = payment_manager(&request);
        let response = PaymentResponse {
            payment_id: PaymentId::new(),
            owner_user_id: request.owner_user_id.clone(),
            agent_account_id: request.agent_account_id.clone(),
            status: PaymentStatus::Succeeded,
            amount_cents: request.amount_cents,
            currency: request.currency,
            ledger_transaction_id: Some(LedgerTransactionId::new()),
            rail_reference: Some("fiat_mock_restored".to_string()),
            failure_reason: None,
            created_at: Utc::now(),
        };
        manager
            .remember_payment_attempt(request.clone(), response.clone())
            .expect("payment attempt should seed idempotency");

        manager.authorizer.token_id = SpendAuthTokenId::new();
        let replayed = manager
            .submit_payment(request)
            .expect("remembered payment should return without authorization");

        assert_eq!(replayed.payment_id, response.payment_id);
        assert_eq!(replayed.rail_reference, response.rail_reference);
        assert_eq!(manager.ledger().list_transactions().unwrap().len(), 0);
    }

    #[test]
    fn mismatched_payment_request_is_rejected_before_rail_and_ledger() {
        let authorized_request = payment_request(PaymentRailKind::FiatMock);
        let mut manager = payment_manager(&authorized_request);
        let mut payment_request = authorized_request;
        payment_request.amount_cents = 2_501;

        let error = manager
            .submit_payment(payment_request)
            .expect_err("amount mismatch should be rejected");

        assert!(matches!(error, PaymentError::AuthorizationRejected { .. }));
        assert!(manager.authorizer.used_payment_id.is_none());
        assert!(manager.payments_by_idempotency_key.is_empty());
    }

    #[test]
    fn failed_rail_payment_does_not_write_money_movement_or_use_token() {
        let mut request = payment_request(PaymentRailKind::FiatMock);
        request.merchant = Some("fail".to_string());
        let mut manager = payment_manager(&request);

        let response = manager
            .submit_payment(request)
            .expect("rail decline should return failed payment response");

        assert_eq!(response.status, PaymentStatus::Failed);
        assert!(response.ledger_transaction_id.is_none());
        assert!(response.failure_reason.is_some());
        assert!(manager.authorizer.used_payment_id.is_none());
    }
}
