//! 数据层：加载、校验、交易日历、int 编码、按日索引存储（架构 §3）。

pub mod benchmark;
pub mod calendar;
pub mod stock_bar;

pub use benchmark::BenchmarkStore;
pub use calendar::TradingCalendar;
pub use stock_bar::StockBarStore;

use std::path::Path;

use crate::error::{BtError, Result};

/// 数据容器：加载行情与基准，`build` 时统一校验并构建交易日历（规范"接口概要"）。
pub struct BTData {
    pub(crate) stock_bar: Option<StockBarStore>,
    pub(crate) benchmark: Option<BenchmarkStore>,
}

impl BTData {
    pub fn new() -> Self {
        Self {
            stock_bar: None,
            benchmark: None,
        }
    }

    /// 加载股票日行情（结构校验随加载完成）。
    pub fn load_stock_bar(mut self, path: &str) -> Result<Self> {
        self.stock_bar = Some(StockBarStore::load(Path::new(path))?);
        Ok(self)
    }

    /// 加载基准收益（一个文件可含多个指数，全部加载）。
    pub fn load_benchmark(mut self, path: &str) -> Result<Self> {
        self.benchmark = Some(BenchmarkStore::load(Path::new(path))?);
        Ok(self)
    }

    /// 统一校验：stock_bar 必需（交易日历来源于它）；benchmark 可后补，
    /// 缺失时 `gen_report` 报错。
    pub fn build(self) -> Result<BTData> {
        if self.stock_bar.is_none() {
            return Err(BtError::Validation(
                "BTData 缺少 stock_bar（交易日历来源），请先 load_stock_bar".into(),
            ));
        }
        Ok(self)
    }
}

impl Default for BTData {
    fn default() -> Self {
        Self::new()
    }
}
