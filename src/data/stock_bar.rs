//! stock_bar 加载、结构校验与按日索引存储（架构 §4.2）。
//!
//! 加载边界完成：`instrument` -> `code` 编码、必需列存在性、重复键、价格 / factor
//! 合法性校验（规范"数据校验"）。非错误类行级规则（停牌、无量、deal_price 列无效）
//! 不在加载期剔除，体现在撮合期可交易性判断。
//!
//! 存储为按 (DayIdx, Code) 排序的 SoA 列式 Vec + 每日行范围索引，主循环零拷贝切片。

use std::collections::HashSet;
use std::path::Path;

use chrono::NaiveDate;
use polars::prelude::*;

use crate::data::calendar::{parse_date, TradingCalendar};
use crate::error::{BtError, Result};
use crate::types::{parse_instrument, Code, DayIdx};

/// stock_bar 必需列（`avg` 与 `vwap` 冗余，当前仅加载 `vwap`，`avg` 不作必需列、也不加载）。
pub const REQUIRED_COLUMNS: &[&str] = &[
    "datetime",
    "instrument",
    "open",
    "close",
    "low",
    "high",
    "volume",
    "money",
    "factor",
    "high_limit",
    "low_limit",
    "pre_close",
    "paused",
    "is_st",
    "vwap",
];

/// stock_bar 存储：按 (DayIdx, Code) 排序的 SoA + 每日行范围索引。
///
/// 缺失值统一以 NaN 表示（价格 / volume / 涨跌停 / pre_close 等）。
pub struct StockBarStore {
    pub calendar: TradingCalendar,
    /// 每行所属交易日
    pub days: Vec<DayIdx>,
    /// 每个交易日 (start_row, len)，O(1) 取当日切片
    pub day_offsets: Vec<(u32, u32)>,
    pub codes: Vec<Code>,
    pub open: Vec<f64>,
    pub close: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub volume: Vec<f64>,
    pub factor: Vec<f64>,
    pub high_limit: Vec<f64>,
    pub low_limit: Vec<f64>,
    pub pre_close: Vec<f64>,
    pub vwap: Vec<f64>,
    pub paused: Vec<bool>,
    pub is_st: Vec<bool>,
    /// 全部出现过的股票 code（信号"instrument 无行情"校验用）
    pub code_set: HashSet<Code>,
}

impl StockBarStore {
    /// 取当日行范围。
    pub fn day_range(&self, day: DayIdx) -> std::ops::Range<usize> {
        let (start, len) = self.day_offsets[day as usize];
        (start as usize)..((start + len) as usize)
    }

    /// 加载并校验 stock_bar.csv。
    pub fn load(path: &Path) -> Result<Self> {
        let df = CsvReadOptions::default()
            .with_has_header(true)
            .try_into_reader_with_file_path(Some(path.to_path_buf()))?
            .finish()?;

        // 必需列存在性
        for name in REQUIRED_COLUMNS {
            if df.column(name).is_err() {
                return Err(BtError::Validation(format!("stock_bar 缺少必需列: {name}")));
            }
        }

        let n = df.height();
        let raw_dates = str_column(&df, "datetime")?;
        let raw_instruments = str_column(&df, "instrument")?;
        let mut open = f64_column(&df, "open")?;
        let mut close = f64_column(&df, "close")?;
        let mut high = f64_column(&df, "high")?;
        let mut low = f64_column(&df, "low")?;
        let mut volume = f64_column(&df, "volume")?;
        let mut factor = f64_column(&df, "factor")?;
        let mut high_limit = f64_column(&df, "high_limit")?;
        let mut low_limit = f64_column(&df, "low_limit")?;
        let mut pre_close = f64_column(&df, "pre_close")?;
        let mut vwap = f64_column(&df, "vwap")?;
        let paused_raw = f64_column(&df, "paused")?;
        let is_st_raw = f64_column(&df, "is_st")?;

        // 日期解析
        let mut dates: Vec<NaiveDate> = Vec::with_capacity(n);
        for s in &raw_dates {
            dates.push(parse_date(s)?);
        }
        // instrument -> code（无法解析按数据校验报错）
        let mut codes: Vec<Code> = Vec::with_capacity(n);
        for s in &raw_instruments {
            codes.push(parse_instrument(s)?);
        }

        // 交易日历 + DayIdx
        let calendar = TradingCalendar::from_dates(dates.clone());
        let mut days: Vec<DayIdx> = Vec::with_capacity(n);
        for d in &dates {
            days.push(calendar.day_idx(d).expect("日历由本批日期构建"));
        }

        // 按 (DayIdx, Code) 排序：打包 u64 键一次 argsort，再统一置换各列
        let keys: Vec<u64> = days
            .iter()
            .zip(&codes)
            .map(|(d, c)| ((*d as u64) << 32) | (*c as u64))
            .collect();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_unstable_by_key(|i| keys[*i]);

        // (datetime, instrument) 重复 -> 报错（排序后相邻即重复）
        for w in order.windows(2) {
            if keys[w[0]] == keys[w[1]] {
                let d = dates[w[0]];
                return Err(BtError::Validation(format!(
                    "stock_bar 存在重复键 (datetime={d}, instrument={})",
                    raw_instruments[w[0]]
                )));
            }
        }

        let permute = |v: &mut Vec<f64>| {
            let old = std::mem::take(v);
            *v = order.iter().map(|i| old[*i]).collect();
        };
        permute(&mut open);
        permute(&mut close);
        permute(&mut high);
        permute(&mut low);
        permute(&mut volume);
        permute(&mut factor);
        permute(&mut high_limit);
        permute(&mut low_limit);
        permute(&mut pre_close);
        permute(&mut vwap);
        let mut paused_raw = paused_raw;
        let mut is_st_raw = is_st_raw;
        permute(&mut paused_raw);
        permute(&mut is_st_raw);
        let old_days = std::mem::take(&mut days);
        let days: Vec<DayIdx> = order.iter().map(|i| old_days[*i]).collect();
        let old_codes = std::mem::take(&mut codes);
        let codes: Vec<Code> = order.iter().map(|i| old_codes[*i]).collect();

        // paused / is_st 缺失按 0 处理并 warning（聚合计数，只打一条日志）
        let mut paused_missing = 0usize;
        let paused: Vec<bool> = paused_raw
            .iter()
            .map(|v| {
                if v.is_nan() {
                    paused_missing += 1;
                    false
                } else {
                    *v != 0.0
                }
            })
            .collect();
        let mut is_st_missing = 0usize;
        let is_st: Vec<bool> = is_st_raw
            .iter()
            .map(|v| {
                if v.is_nan() {
                    is_st_missing += 1;
                    false
                } else {
                    *v != 0.0
                }
            })
            .collect();
        if paused_missing > 0 {
            log::warn!("stock_bar: paused 缺失 {paused_missing} 行，按 0 处理");
        }
        if is_st_missing > 0 {
            log::warn!("stock_bar: is_st 缺失 {is_st_missing} 行，按 0 处理");
        }

        // 行级硬校验（缺失值以 NaN 表示，仅校验非缺失值）
        for (i, &c) in codes.iter().enumerate() {
            for (name, col) in [
                ("open", &open),
                ("close", &close),
                ("high", &high),
                ("low", &low),
            ] {
                let v = col[i];
                if !v.is_nan() && v <= 0.0 {
                    return Err(BtError::Validation(format!(
                        "stock_bar 行 {i}（code={c}）{name} = {v} <= 0"
                    )));
                }
            }
            if !high[i].is_nan() && !low[i].is_nan() && high[i] < low[i] {
                return Err(BtError::Validation(format!(
                    "stock_bar 行 {i}（code={c}）high({}) < low({})",
                    high[i], low[i]
                )));
            }
            if !volume[i].is_nan() && volume[i] < 0.0 {
                return Err(BtError::Validation(format!(
                    "stock_bar 行 {i}（code={c}）volume = {} < 0",
                    volume[i]
                )));
            }
            // factor 缺失 / NaN / <= 0 -> 报错（复权依赖该列）
            if factor[i].is_nan() || factor[i] <= 0.0 {
                return Err(BtError::Validation(format!(
                    "stock_bar 行 {i}（code={c}）factor = {} 非法（缺失/NaN/<=0）",
                    factor[i]
                )));
            }
        }

        // 每日行范围索引（已按 DayIdx 排序）
        let mut day_offsets = vec![(0u32, 0u32); calendar.len()];
        let mut start = 0usize;
        for i in 1..=n {
            if i == n || days[i] != days[start] {
                day_offsets[days[start] as usize] = (start as u32, (i - start) as u32);
                start = i;
            }
        }

        let code_set: HashSet<Code> = codes.iter().copied().collect();

        Ok(Self {
            calendar,
            days,
            day_offsets,
            codes,
            open,
            close,
            high,
            low,
            volume,
            factor,
            high_limit,
            low_limit,
            pre_close,
            vwap,
            paused,
            is_st,
            code_set,
        })
    }
}

/// 读取字符串列；非 String 类型先尝试 cast（如 Date -> String）。
fn str_column(df: &DataFrame, name: &str) -> Result<Vec<String>> {
    let col = df
        .column(name)
        .map_err(|_| BtError::Validation(format!("缺少列: {name}")))?;
    let col = if col.dtype() != &DataType::String {
        col.cast(&DataType::String)
            .map_err(|e| BtError::Validation(format!("列 {name} 无法转为字符串: {e}")))?
    } else {
        col.clone()
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

/// 读取数值列（统一 cast 到 f64；缺失 -> NaN）。
fn f64_column(df: &DataFrame, name: &str) -> Result<Vec<f64>> {
    let col = df
        .column(name)
        .map_err(|_| BtError::Validation(format!("缺少列: {name}")))?;
    let col = col
        .cast(&DataType::Float64)
        .map_err(|e| BtError::Validation(format!("列 {name} 无法转为数值: {e}")))?;
    let ca = col
        .f64()
        .map_err(|e| BtError::Validation(format!("列 {name} 非数值类型: {e}")))?;
    Ok(ca.iter().map(|o| o.unwrap_or(f64::NAN)).collect())
}
