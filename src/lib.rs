//! Rust 股票回测系统（依据 doc/specification.md 与 doc/architecture.md 实现）。
//!
//! 公开 Facade 与规范"使用方法"一致：`load_signal` / `BTData` / `Account` /
//! `Exchange` / `Backtest` / `BTResult` / `Report`。

pub mod account;
pub mod backtest;
pub mod data;
pub mod error;
pub mod exchange;
pub mod order;
pub mod position;
pub mod report;
pub mod result;
pub mod signal;
pub mod strategy;
pub mod types;

pub use account::{Account, DailyRecord, HistPositionRow};
pub use backtest::Backtest;
pub use data::BTData;
pub use error::{BtError, Result};
pub use exchange::Exchange;
pub use order::{Decision, Order, Side, TradeRecord};
pub use position::{PositionEntry, Positions};
pub use report::{DerivedStats, Report};
pub use result::BTResult;
pub use signal::{load_signal, Signal, SignalDay};
pub use strategy::{PostSellContext, Strategy, StrategyContext, TopkDropoutStrategy};
pub use types::{
    format_instrument, parse_instrument, BenchmarkName, Code, DayIdx, DealPrice, ExcessMethod,
    StockTradable, TradableInfo,
};
