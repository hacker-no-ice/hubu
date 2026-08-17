use chrono::{DateTime, Utc};

/// Half-open UTC time window used by policies and budget limits.
///
/// The start is included and the end, when present, is excluded:
/// `[starting_at, ending_before)`. Leaving `ending_before` unset represents an
/// open-ended period.
#[derive(Debug, Clone)]
pub struct TimePeriod {
    pub starting_at: DateTime<Utc>,
    pub ending_before: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimePeriodError {
    EndMustBeAfterStart,
}

impl TimePeriod {
    pub fn new(
        starting_at: DateTime<Utc>,
        ending_before: Option<DateTime<Utc>>,
    ) -> Result<Self, TimePeriodError> {
        if let Some(end) = ending_before {
            if end <= starting_at {
                return Err(TimePeriodError::EndMustBeAfterStart);
            }
        }

        Ok(Self {
            starting_at,
            ending_before,
        })
    }

    pub fn contains(&self, at: DateTime<Utc>) -> bool {
        at >= self.starting_at && self.ending_before.is_none_or(|end| at < end)
    }

    pub fn is_open_ended(&self) -> bool {
        self.ending_before.is_none()
    }
}
