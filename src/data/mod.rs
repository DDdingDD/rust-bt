//! 数据层：加载、校验、交易日历、int 编码、按日索引存储（架构 §3）。

pub mod benchmark;
pub mod calendar;
pub mod stock_bar;

pub use benchmark::BenchmarkStore;
pub use calendar::TradingCalendar;
pub use stock_bar::StockBarStore;

use std::path::Path;

use polars::prelude::*;

use crate::error::{BtError, Result};

/// 按扩展名读取 DataFrame：`.parquet` / `.pq` 走 parquet，其余（含无扩展名）按 CSV。
///
/// 两种格式的列要求与校验完全一致（规范"数据文件格式"），parquet 仅是另一种
/// 物理存储；类型化日期列（Date / Datetime）由 [`date_strings`] 统一转换。
pub(crate) fn read_dataframe(path: &Path) -> Result<DataFrame> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let df = match ext.as_str() {
        "parquet" | "pq" => {
            let file = std::fs::File::open(path)?;
            ParquetReader::new(file).finish()?
        }
        _ => CsvReadOptions::default()
            .with_has_header(true)
            .try_into_reader_with_file_path(Some(path.to_path_buf()))?
            .finish()?,
    };
    Ok(df)
}

/// 读取日期列（`datetime`）为 `YYYY-MM-DD` 字符串。
///
/// CSV 中日期恒为字符串；parquet 中常为类型化列：`Date` 直接转字符串，
/// `Datetime` 先截断到日（时间部分丢弃），其余类型尝试 cast。缺失行报错。
pub(crate) fn date_strings(df: &DataFrame, name: &str) -> Result<Vec<String>> {
    let col = df
        .column(name)
        .map_err(|_| BtError::Validation(format!("缺少列: {name}")))?;
    let col = match col.dtype() {
        DataType::String => col.clone(),
        DataType::Datetime(_, _) => col
            .cast(&DataType::Date)
            .and_then(|c| c.cast(&DataType::String))
            .map_err(|e| BtError::Validation(format!("列 {name} 无法转为日期字符串: {e}")))?,
        _ => col
            .cast(&DataType::String)
            .map_err(|e| BtError::Validation(format!("列 {name} 无法转为字符串: {e}")))?,
    };
    let ca = col
        .str()
        .map_err(|e| BtError::Validation(format!("列 {name} 非字符串类型: {e}")))?;
    ca.iter()
        .enumerate()
        .map(|(i, o)| {
            o.map(str::to_owned)
                .ok_or_else(|| BtError::Validation(format!("列 {name} 第 {i} 行缺失")))
        })
        .collect()
}

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
