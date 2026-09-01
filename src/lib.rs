//! Rust 股票回测系统（依据 doc/specification.md 与 doc/architecture.md 实现）。
//!
//! 公开 Facade 与规范"使用方法"一致：`load_signal` / `BTData` / `Account` /
//! `Exchange` / `Backtest` / `BTResult` / `Report`。
//! 嵌入其他 Rust 代码优先用高层便捷层 `api::run`（一次调用完成装配与回测，
//! 参数类型化）；组件 Facade 供细粒度编排，两层共用同一撮合与估值路径。

pub mod account;
pub mod api;
pub mod backtest;
pub mod config;
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
pub use api::{run, run_from_signal_file, signal_from_pairs, BtOutput, BtParams, ExchangeParams, ExportNames, StrategySpec};
pub use backtest::Backtest;
pub use config::BtConfig;
pub use data::BTData;
pub use error::{BtError, Result};
pub use exchange::Exchange;
pub use order::{Decision, Order, Side, TradeRecord};
pub use position::{PositionEntry, Positions};
pub use report::{DerivedStats, Report};
pub use result::BTResult;
pub use signal::{load_signal, signal_from_dataframe, Signal, SignalDay};
pub use strategy::{PostSellContext, Strategy, StrategyContext, TopkDropoutStrategy, TopkStrategy};
pub use types::{
    format_instrument, parse_instrument, BenchmarkName, Code, DayIdx, DealPrice, ExcessMethod,
    StockTradable, TradableInfo, WapKind,
};
