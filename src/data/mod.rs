//! 数据层：加载、校验、交易日历、int 编码、按日索引存储（架构 §3）。

pub mod benchmark;
pub mod calendar;
pub mod stock_bar;
pub mod wap;

pub use benchmark::BenchmarkStore;
pub use calendar::TradingCalendar;
pub use stock_bar::StockBarStore;
pub use wap::WapStore;

use std::path::Path;
use std::sync::Arc;

use polars::prelude::*;

use crate::error::{BtError, Result};

/// 目录发现：在 `dir` 中按文件名主干 `stem` 查找数据文件，扩展名优先级
/// `.parquet` > `.pq` > `.csv`。找到返回完整路径，未找到返回 `None`。
///
/// 供 `data.dir` 目录配置（`BtConfig`）与 `DataSource::Dir`（嵌入 API）共用；
/// 同一主干多格式并存时按优先级静默取高优先级格式（parquet 与 CSV 内容
/// 口径一致，仅物理存储不同，见 [`read_dataframe`]）。
pub fn find_data_file(dir: &str, stem: &str) -> Option<String> {
    let dir = Path::new(dir);
    for ext in ["parquet", "pq", "csv"] {
        let path = dir.join(format!("{stem}.{ext}"));
        if path.is_file() {
            return Some(path.to_string_lossy().into_owned());
        }
    }
    None
}

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

/// 按扩展名读取 DataFrame 的指定列子集（列名缺失由调用方按必需列校验报错）。
///
/// parquet 走列投影（wap 数据 90 列 2.7GB，只读所需 8 列）；CSV 无投影则全读后
/// select。`columns` 为空时等价于 [`read_dataframe`]。
pub(crate) fn read_dataframe_columns(path: &Path, columns: &[String]) -> Result<DataFrame> {
    if columns.is_empty() {
        return read_dataframe(path);
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let df = match ext.as_str() {
        "parquet" | "pq" => {
            let file = std::fs::File::open(path)?;
            ParquetReader::new(file)
                .with_columns(Some(columns.to_vec()))
                .finish()?
        }
        _ => {
            let df = CsvReadOptions::default()
                .with_has_header(true)
                .try_into_reader_with_file_path(Some(path.to_path_buf()))?
                .finish()?;
            df.select(columns)
                .map_err(|e| BtError::Validation(format!("列选择失败: {e}")))?
        }
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
///
/// 内部以 `Arc` 持有三份存储（决策 D15）：`Clone` 为 Arc 计数克隆（廉价），
/// 同一份已加载数据可复用于多次回测（参数扫描免重载）；依赖撮合参数的派生列
/// 不随 `BTData` 共享，每次装配 `Backtest` 时按 `deal_price` 等重建。
#[derive(Clone)]
pub struct BTData {
    pub(crate) stock_bar: Option<Arc<StockBarStore>>,
    pub(crate) benchmark: Option<Arc<BenchmarkStore>>,
    /// 时段 VWAP/TWAP 数据（`deal_price = vwapN / twapN` 时必需，装配期校验）
    pub(crate) wap: Option<Arc<WapStore>>,
}

impl BTData {
    pub fn new() -> Self {
        Self {
            stock_bar: None,
            benchmark: None,
            wap: None,
        }
    }

    /// 加载股票日行情（结构校验随加载完成）。
    pub fn load_stock_bar(mut self, path: &str) -> Result<Self> {
        self.stock_bar = Some(Arc::new(StockBarStore::load(Path::new(path))?));
        Ok(self)
    }

    /// 加载基准收益（一个文件可含多个指数，全部加载）。
    pub fn load_benchmark(mut self, path: &str) -> Result<Self> {
        self.benchmark = Some(Arc::new(BenchmarkStore::load(Path::new(path))?));
        Ok(self)
    }

    /// 加载 wap 时段数据（CSV 或 parquet）。`window` 为时段号 1..=11，须与
    /// `deal_price` 的时段一致（`Backtest::new` 装配期校验，不一致报错）。
    pub fn load_wap(mut self, path: &str, window: u8) -> Result<Self> {
        self.wap = Some(Arc::new(WapStore::load(Path::new(path), window)?));
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

/// 不展开存储内容（数百 MB 级），只标注各数据是否已加载。
impl std::fmt::Debug for BTData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BTData")
            .field("stock_bar", &self.stock_bar.is_some())
            .field("benchmark", &self.benchmark.is_some())
            .field("wap", &self.wap.as_ref().map(|w| w.window))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_data_file_extension_priority() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        assert!(find_data_file(d, "stock_bar").is_none());

        // 只有 csv 时命中 csv
        std::fs::write(dir.path().join("stock_bar.csv"), "x").unwrap();
        assert!(find_data_file(d, "stock_bar")
            .unwrap()
            .ends_with("stock_bar.csv"));

        // 并存时 parquet 优先于 pq 优先于 csv
        std::fs::write(dir.path().join("stock_bar.pq"), "x").unwrap();
        assert!(find_data_file(d, "stock_bar")
            .unwrap()
            .ends_with("stock_bar.pq"));
        std::fs::write(dir.path().join("stock_bar.parquet"), "x").unwrap();
        assert!(find_data_file(d, "stock_bar")
            .unwrap()
            .ends_with("stock_bar.parquet"));

        // 主干精确匹配：前缀相似的文件不误命中
        assert!(find_data_file(d, "benchmark").is_none());
        std::fs::write(dir.path().join("benchmark_1min.csv"), "x").unwrap();
        assert!(find_data_file(d, "benchmark").is_none());

        // 目录而非文件不命中
        std::fs::create_dir(dir.path().join("wap.csv")).unwrap();
        assert!(find_data_file(d, "wap").is_none());
    }
}
