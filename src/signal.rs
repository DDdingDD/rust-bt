//! Signal（信号，架构 §4.3）：pred.csv 加载、结构校验、剥离 ret、按日索引。
//!
//! 可见性约束：`ret` 列仅供离线信号评估，加载后即剥离——`Signal` 结构上
//! 不存在该字段，回测引擎与策略不可见（防前视，架构 D4）。
//!
//! 校验分两阶段（规范"数据校验"）：
//! - 加载期（本模块）：(datetime, instrument) 重复 -> 报错；score 缺失 / NaN ->
//!   丢弃 + warning；instrument 无法按 SH/SZ 规则编码 -> 丢弃 + warning；
//! - 推迟到 `Backtest::run` 启动时：datetime 不在交易日历、instrument 无行情。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use chrono::NaiveDate;
use polars::prelude::*;

use crate::data::calendar::parse_date;
use crate::error::{BtError, Result};
use crate::types::{parse_instrument, Code};

/// 单个信号日的 (code, score) 集合。按 score 无序，策略自排序。
#[derive(Clone, Debug)]
pub struct SignalDay {
    pub codes: Vec<Code>,
    pub scores: Vec<f64>,
}

impl SignalDay {
    /// code -> score 反查。
    pub fn as_map(&self) -> HashMap<Code, f64> {
        self.codes.iter().copied().zip(self.scores.iter().copied()).collect()
    }

    pub fn len(&self) -> usize {
        self.codes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }
}

/// 信号容器：按日期索引。`ret` 已在加载时剥离，结构上不存在。
pub struct Signal {
    pub(crate) days: BTreeMap<NaiveDate, SignalDay>,
}

impl Signal {
    /// 取某日信号。
    pub fn get(&self, date: &NaiveDate) -> Option<&SignalDay> {
        self.days.get(date)
    }
}

/// 加载 pred.csv：结构校验并剥离 `ret`。
///
/// 日历 / 行情相关校验依赖交易日历，推迟到 `Backtest::run` 启动时执行。
pub fn load_signal(path: &str) -> Result<Signal> {
    let df = CsvReadOptions::default()
        .with_has_header(true)
        .try_into_reader_with_file_path(Some(Path::new(path).to_path_buf()))?
        .finish()?;

    for name in ["datetime", "instrument", "score"] {
        if df.column(name).is_err() {
            return Err(BtError::Validation(format!("pred 缺少必需列: {name}")));
        }
    }

    let raw_dates = df
        .column("datetime")?
        .cast(&DataType::String)?
        .str()?
        .iter()
        .map(|o| o.map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| BtError::Validation("pred datetime 存在缺失".into()))?;
    let instruments = df
        .column("instrument")?
        .cast(&DataType::String)?
        .str()?
        .iter()
        .map(|o| o.map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| BtError::Validation("pred instrument 存在缺失".into()))?;
    let scores = df
        .column("score")?
        .cast(&DataType::Float64)?
        .f64()?
        .iter()
        .map(|o| o.unwrap_or(f64::NAN))
        .collect::<Vec<_>>();
    // ret 列在此显式剥离：读取后不回灌（本期仅隔离，为 IC 分析预留）。

    let n = raw_dates.len();
    let mut dates = Vec::with_capacity(n);
    for s in &raw_dates {
        dates.push(parse_date(s)?);
    }

    // (datetime, instrument) 重复 -> 报错
    let mut seen = std::collections::HashSet::with_capacity(n);
    for (d, inst) in dates.iter().zip(&instruments) {
        if !seen.insert((*d, inst.as_str())) {
            return Err(BtError::Validation(format!(
                "pred 存在重复键 (datetime={d}, instrument={inst})"
            )));
        }
    }

    let mut days: BTreeMap<NaiveDate, SignalDay> = BTreeMap::new();
    let mut dropped_score = 0usize;
    let mut dropped_instrument = 0usize;
    for ((d, inst), score) in dates.iter().zip(&instruments).zip(&scores) {
        // score 为 NaN / 缺失 -> 丢弃 + warning
        if score.is_nan() {
            dropped_score += 1;
            continue;
        }
        // instrument 无法编码（非 SH/SZ 股票代码）-> 丢弃 + warning
        let code = match parse_instrument(inst) {
            Ok(c) => c,
            Err(_) => {
                dropped_instrument += 1;
                continue;
            }
        };
        let entry = days.entry(*d).or_insert_with(|| SignalDay {
            codes: Vec::new(),
            scores: Vec::new(),
        });
        entry.codes.push(code);
        entry.scores.push(*score);
    }
    if dropped_score > 0 {
        log::warn!("pred: score 缺失/NaN，丢弃 {dropped_score} 条信号");
    }
    if dropped_instrument > 0 {
        log::warn!("pred: instrument 无法按 SH/SZ 规则编码，丢弃 {dropped_instrument} 条信号");
    }

    Ok(Signal { days })
}
