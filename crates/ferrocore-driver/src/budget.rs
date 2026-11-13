//! Fail-closed per-session token, turn and optional cost accounting.
//!
//! Record each completed response before deciding whether to execute its action.
//! The caller must persist the response and usage even when accounting fails.

use crate::llm::UsageStatus;
use std::fmt;

const MILLION: u128 = 1_000_000;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetConfig {
    pub max_turns: u64,
    pub max_total_tokens: u64,
    pub max_cost_micros_usd: Option<u64>,
    pub input_cost_per_million_micros: Option<u64>,
    pub output_cost_per_million_micros: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetError {
    InvalidLimit,
    MissingPrice,
    InvalidUsage,
    UsageUnavailable,
    ArithmeticOverflow,
    BudgetExceeded,
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLimit => "budget limits must be greater than zero",
            Self::MissingPrice => "cost limit and both token prices must be configured together",
            Self::InvalidUsage => "token usage total does not match prompt and completion tokens",
            Self::UsageUnavailable => "token usage is unavailable",
            Self::ArithmeticOverflow => "budget accounting overflow",
            Self::BudgetExceeded => "budget exceeded",
        })
    }
}
impl std::error::Error for BudgetError {}

#[derive(Debug, Clone)]
pub struct Budget {
    config: BudgetConfig,
    turns: u64,
    total_tokens: u64,
    cost_micros_usd: u64,
    stopped: Option<BudgetError>,
}

impl Budget {
    pub fn new(config: BudgetConfig) -> Result<Self, BudgetError> {
        if config.max_turns == 0 || config.max_total_tokens == 0 || config.max_cost_micros_usd == Some(0) {
            return Err(BudgetError::InvalidLimit);
        }
        match (config.max_cost_micros_usd, config.input_cost_per_million_micros, config.output_cost_per_million_micros) {
            (None, None, None) | (Some(_), Some(_), Some(_)) => {},
            _ => return Err(BudgetError::MissingPrice),
        }
        Ok(Self { config, turns: 0, total_tokens: 0, cost_micros_usd: 0, stopped: None })
    }

    pub fn turns(&self) -> u64 { self.turns }
    pub fn total_tokens(&self) -> u64 { self.total_tokens }
    /// `None` means no cost limit and prices were configured, not zero cost.
    pub fn cost_micros_usd(&self) -> Option<u64> {
        self.config.max_cost_micros_usd.map(|_| self.cost_micros_usd)
    }

    /// Call before starting a request. Equality leaves no budget for another turn.
    pub fn check_before_turn(&self) -> Result<(), BudgetError> {
        if let Some(error) = self.stopped { return Err(error); }
        if self.turns >= self.config.max_turns || self.total_tokens >= self.config.max_total_tokens
            || self.config.max_cost_micros_usd.is_some_and(|max| self.cost_micros_usd >= max)
        {
            return Err(BudgetError::BudgetExceeded);
        }
        Ok(())
    }

    /// Add a completed response. A response exactly at a limit is allowed, but
    /// another turn is not. Missing usage or arithmetic failure permanently stops
    /// this accountant: the unknown usage cannot safely be recovered by retrying.
    pub fn record_response(&mut self, usage: UsageStatus) -> Result<(), BudgetError> {
        self.check_before_turn()?;
        let result = self.add_response(usage);
        if let Err(error) = result { self.stopped = Some(error); }
        result
    }

    fn add_response(&mut self, usage: UsageStatus) -> Result<(), BudgetError> {
        let usage = match usage {
            UsageStatus::Available(usage) => usage,
            UsageStatus::Unavailable => return Err(BudgetError::UsageUnavailable),
        };
        let turns = self.turns.checked_add(1).ok_or(BudgetError::ArithmeticOverflow)?;
        let turn_tokens = usage.prompt_tokens.checked_add(usage.completion_tokens)
            .ok_or(BudgetError::ArithmeticOverflow)?;
        if turn_tokens != usage.total_tokens {
            return Err(BudgetError::InvalidUsage);
        }
        let tokens = self.total_tokens.checked_add(turn_tokens).ok_or(BudgetError::ArithmeticOverflow)?;
        let cost = match (self.config.input_cost_per_million_micros, self.config.output_cost_per_million_micros) {
            (Some(input), Some(output)) => {
                let numerator = u128::from(usage.prompt_tokens).checked_mul(u128::from(input))
                    .and_then(|a| u128::from(usage.completion_tokens).checked_mul(u128::from(output)).and_then(|b| a.checked_add(b)))
                    .ok_or(BudgetError::ArithmeticOverflow)?;
                let rounded = numerator / MILLION + u128::from(numerator % MILLION != 0);
                self.cost_micros_usd.checked_add(u64::try_from(rounded).map_err(|_| BudgetError::ArithmeticOverflow)?)
                    .ok_or(BudgetError::ArithmeticOverflow)?
            }
            _ => 0,
        };
        self.turns = turns;
        self.total_tokens = tokens;
        self.cost_micros_usd = cost;
        if turns > self.config.max_turns || tokens > self.config.max_total_tokens
            || self.config.max_cost_micros_usd.is_some_and(|max| cost > max)
        {
            return Err(BudgetError::BudgetExceeded);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::TokenUsage;

    fn config() -> BudgetConfig {
        BudgetConfig { max_turns: 2, max_total_tokens: 10, max_cost_micros_usd: None,
            input_cost_per_million_micros: None, output_cost_per_million_micros: None }
    }
    fn usage(prompt_tokens: u64, completion_tokens: u64) -> UsageStatus {
        UsageStatus::Available(TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        })
    }

    #[test]
    fn exact_limits_allow_response_but_not_next_turn() {
        let mut b = Budget::new(config()).unwrap();
        b.record_response(usage(4, 1)).unwrap();
        b.check_before_turn().unwrap();
        b.record_response(usage(4, 1)).unwrap();
        assert_eq!((b.turns(), b.total_tokens()), (2, 10));
        assert_eq!(b.check_before_turn(), Err(BudgetError::BudgetExceeded));
        assert_eq!(b.record_response(usage(0, 0)), Err(BudgetError::BudgetExceeded));
        assert_eq!((b.turns(), b.total_tokens()), (2, 10));
        assert_eq!(b.cost_micros_usd(), None);
    }

    #[test]
    fn exceeding_tokens_records_usage_and_stops() {
        let mut b = Budget::new(config()).unwrap();
        assert_eq!(b.record_response(usage(8, 3)), Err(BudgetError::BudgetExceeded));
        assert_eq!((b.turns(), b.total_tokens()), (1, 11));
        assert_eq!(b.check_before_turn(), Err(BudgetError::BudgetExceeded));
    }

    #[test]
    fn missing_usage_permanently_stops() {
        let mut b = Budget::new(config()).unwrap();
        assert_eq!(b.record_response(UsageStatus::Unavailable), Err(BudgetError::UsageUnavailable));
        assert_eq!(b.check_before_turn(), Err(BudgetError::UsageUnavailable));
        assert_eq!(b.record_response(usage(1, 1)), Err(BudgetError::UsageUnavailable));
        assert_eq!((b.turns(), b.total_tokens()), (0, 0));
    }

    #[test]
    fn inconsistent_total_tokens_permanently_stops() {
        let mut b = Budget::new(config()).unwrap();
        let inconsistent = UsageStatus::Available(TokenUsage {
            prompt_tokens: 4,
            completion_tokens: 1,
            total_tokens: 6,
        });
        assert_eq!(b.record_response(inconsistent), Err(BudgetError::InvalidUsage));
        assert_eq!(b.check_before_turn(), Err(BudgetError::InvalidUsage));
        assert_eq!((b.turns(), b.total_tokens()), (0, 0));
    }

    #[test]
    fn incomplete_prices_and_zero_limits_are_errors() {
        let mut c = config();
        c.max_cost_micros_usd = Some(1);
        assert_eq!(Budget::new(c).unwrap_err(), BudgetError::MissingPrice);
        c.input_cost_per_million_micros = Some(1);
        assert_eq!(Budget::new(c).unwrap_err(), BudgetError::MissingPrice);
        c.max_cost_micros_usd = None;
        c.output_cost_per_million_micros = Some(1);
        assert_eq!(Budget::new(c).unwrap_err(), BudgetError::MissingPrice);
        c.max_cost_micros_usd = Some(1);
        c.max_turns = 0;
        assert_eq!(Budget::new(c).unwrap_err(), BudgetError::InvalidLimit);
        c.max_turns = 1;
        c.max_total_tokens = 0;
        assert_eq!(Budget::new(c).unwrap_err(), BudgetError::InvalidLimit);
        c.max_total_tokens = 1;
        c.max_cost_micros_usd = Some(0);
        assert_eq!(Budget::new(c).unwrap_err(), BudgetError::InvalidLimit);
        c.max_cost_micros_usd = Some(1);
        c.input_cost_per_million_micros = Some(0);
        c.output_cost_per_million_micros = Some(0);
        let mut b = Budget::new(c).unwrap();
        b.record_response(usage(1, 0)).unwrap();
        assert_eq!(b.cost_micros_usd(), Some(0));
    }

    #[test]
    fn cost_rounds_combined_charge_up_per_response() {
        let mut c = config(); c.max_cost_micros_usd = Some(3);
        c.input_cost_per_million_micros = Some(400_000);
        c.output_cost_per_million_micros = Some(600_000);
        let mut b = Budget::new(c).unwrap();
        b.record_response(usage(1, 1)).unwrap();
        assert_eq!(b.cost_micros_usd(), Some(1));
        b.record_response(usage(1, 2)).unwrap();
        assert_eq!(b.cost_micros_usd(), Some(3));
        assert_eq!(b.check_before_turn(), Err(BudgetError::BudgetExceeded));
    }

    #[test]
    fn exceeding_cost_stops_and_retains_accounting() {
        let mut c = config(); c.max_cost_micros_usd = Some(1);
        c.input_cost_per_million_micros = Some(1_000_001);
        c.output_cost_per_million_micros = Some(0);
        let mut b = Budget::new(c).unwrap();
        assert_eq!(b.record_response(usage(1, 0)), Err(BudgetError::BudgetExceeded));
        assert_eq!(b.cost_micros_usd(), Some(2));
        assert_eq!(b.check_before_turn(), Err(BudgetError::BudgetExceeded));
    }

    #[test]
    fn ceiling_rounding_exact_boundaries() {
        let mut c = config();
        c.max_turns = 10;
        c.max_total_tokens = 2_000_000;
        c.max_cost_micros_usd = Some(10);
        c.input_cost_per_million_micros = Some(1);
        c.output_cost_per_million_micros = Some(1_000_000);
        let mut b = Budget::new(c).unwrap();
        b.record_response(usage(0, 0)).unwrap();
        assert_eq!(b.cost_micros_usd(), Some(0));
        b.record_response(usage(1, 0)).unwrap();
        assert_eq!(b.cost_micros_usd(), Some(1));
        b.record_response(usage(999_999, 0)).unwrap();
        assert_eq!(b.cost_micros_usd(), Some(2));
    }

    #[test]
    fn overflow_is_explicit_and_terminal() {
        let mut c = config();
        c.max_total_tokens = u64::MAX;
        let mut b = Budget::new(c).unwrap();
        assert_eq!(b.record_response(usage(u64::MAX, 1)), Err(BudgetError::ArithmeticOverflow));
        assert_eq!(b.check_before_turn(), Err(BudgetError::ArithmeticOverflow));

        c.max_cost_micros_usd = Some(u64::MAX);
        c.input_cost_per_million_micros = Some(u64::MAX);
        c.output_cost_per_million_micros = Some(u64::MAX);
        let mut b = Budget::new(c).unwrap();
        assert_eq!(b.record_response(usage(u64::MAX, u64::MAX)), Err(BudgetError::ArithmeticOverflow));
        assert_eq!(b.check_before_turn(), Err(BudgetError::ArithmeticOverflow));

        let mut b = Budget::new(c).unwrap();
        assert_eq!(b.record_response(usage(u64::MAX / 2, u64::MAX / 2)), Err(BudgetError::ArithmeticOverflow));

        c.max_turns = 2;
        c.input_cost_per_million_micros = Some(2_000_000);
        c.output_cost_per_million_micros = Some(0);
        let mut b = Budget::new(c).unwrap();
        b.record_response(usage(u64::MAX / 2, 0)).unwrap();
        assert_eq!(b.record_response(usage(1, 0)), Err(BudgetError::ArithmeticOverflow));
        assert_eq!(b.check_before_turn(), Err(BudgetError::ArithmeticOverflow));
    }
}
