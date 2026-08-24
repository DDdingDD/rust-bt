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

    /// 从 `(instrument, score)` 对内存构造（嵌入方程序化生成信号）。
    ///
    /// 校验口径与 `load_signal` 一致：同日 instrument 重复 -> `Err`；
    /// instrument 无法解析 / score 非有限 -> 丢弃 + warning。
    /// 日历与行情相关校验（无行情的 instrument）同样推迟到 `Backtest::run` 启动时。
    pub fn from_pairs(pairs: Vec<(String, f64)>) -> Result<Self> {
        let mut seen = std::collections::HashSet::with_capacity(pairs.len());
        let mut codes = Vec::with_capacity(pairs.len());
        let mut scores = Vec::with_capacity(pairs.len());
        for (instrument, score) in pairs {
            let code = match parse_instrument(&instrument) {
                Ok(c) => c,
                Err(_) => {
                    log::warn!("pred: instrument 无法解析，丢弃: {instrument}");
                    continue;
                }
            };
            if !seen.insert(code) {
                return Err(BtError::Validation(format!(
                    "pred 存在重复 instrument: {instrument}"
                )));
            }
            if score.is_finite() {
                codes.push(code);
                scores.push(score);
            } else {
                log::warn!("pred: score 非有限值，丢弃: {instrument} score={score}");
            }
        }
        Ok(Self { codes, scores })
    }
}

/// 信号容器：按日期索引。`ret` 已在加载时剥离，结构上不存在。
#[derive(Clone, Debug)]
pub struct Signal {
    pub(crate) days: BTreeMap<NaiveDate, SignalDay>,
}

impl Signal {
    /// 取某日信号。
    pub fn get(&self, date: &NaiveDate) -> Option<&SignalDay> {
        self.days.get(date)
    }

    /// 全部信号日（升序）。`ret` 剥离后仍可枚举日期供离线分析。
    pub fn dates(&self) -> impl Iterator<Item = NaiveDate> + '_ {
        self.days.keys().copied()
    }

    /// 从内存构造（配合 `SignalDay::from_pairs`；逐日校验已在前者完成）。
    pub fn from_days(days: BTreeMap<NaiveDate, SignalDay>) -> Self {
        Self { days }
    }
}

/// 加载 pred.csv：结构校验并剥离 `ret`（内部转发 `signal_from_dataframe`）。
///
/// 日历 / 行情相关校验依赖交易日历，推迟到 `Backtest::run` 启动时执行。
pub fn load_signal(path: &str) -> Result<Signal> {
    let df = CsvReadOptions::default()
        .with_has_header(true)
        .try_into_reader_with_file_path(Some(Path::new(path).to_path_buf()))?
        .finish()?;
    signal_from_dataframe(&df)
}

/// 从内存 DataFrame 构造信号（嵌入方 polars 管线直连）。
///
/// 列要求与 pred.csv 相同：必需 `datetime` / `instrument` / `score` 三列
/// （datetime 与 instrument cast 为 String、score cast 为 f64，cast 失败返回
/// polars 错误）；多余列忽略--`ret` 同样被剥离，结构上不进入回测（防前视，D4）。
/// 校验口径与 `load_signal` 完全一致（同一实现）。
///
/// 日历 / 行情相关校验依赖交易日历，推迟到 `Backtest::run` 启动时执行。
pub fn signal_from_dataframe(df: &DataFrame) -> Result<Signal> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pred_df(dates: Vec<&str>, insts: Vec<&str>, scores: Vec<f64>, rets: Vec<f64>) -> DataFrame {
        DataFrame::new(vec![
            Series::new("datetime".into(), dates).into(),
            Series::new("instrument".into(), insts).into(),
            Series::new("score".into(), scores).into(),
            Series::new("ret".into(), rets).into(),
        ])
        .unwrap()
    }

    #[test]
    fn from_dataframe_parses_and_strips_ret() {
        let df = pred_df(
            vec!["2026-01-05", "2026-01-05", "2026-01-06"],
            vec!["SH600001", "SH600002", "SH600001"],
            vec![3.0, 2.0, 1.0],
            vec![0.1, 0.2, 0.3],
        );
        let sig = signal_from_dataframe(&df).unwrap();

        // ret 剥离：Signal 结构上只有 codes/scores
        let d1 = sig.get(&parse_date("2026-01-05").unwrap()).unwrap();
        assert_eq!(d1.codes, vec![600001, 600002]);
        assert_eq!(d1.scores, vec![3.0, 2.0]);
        let d2 = sig.get(&parse_date("2026-01-06").unwrap()).unwrap();
        assert_eq!(d2.codes, vec![600001]);
        // 日期可枚举
        assert_eq!(sig.dates().count(), 2);
    }

    #[test]
    fn from_dataframe_drops_invalid_and_errors_on_duplicate() {
        // 不可编码 instrument / NaN score -> 丢弃（口径同 CSV 加载）
        let df = pred_df(
            vec!["2026-01-05", "2026-01-05", "2026-01-05"],
            vec!["SH600001", "BJ832000", "SH600002"],
            vec![1.0, 2.0, f64::NAN],
            vec![0.0; 3],
        );
        let sig = signal_from_dataframe(&df).unwrap();
        let day = sig.get(&parse_date("2026-01-05").unwrap()).unwrap();
        assert_eq!(day.codes, vec![600001]);

        // (datetime, instrument) 重复 -> Err
        let df = pred_df(
            vec!["2026-01-05", "2026-01-05"],
            vec!["SH600001", "SH600001"],
            vec![1.0, 2.0],
            vec![0.0; 2],
        );
        let err = signal_from_dataframe(&df).unwrap_err();
        assert!(err.to_string().contains("重复"), "{err}");

        // 缺必需列 -> Err
        let df =
            DataFrame::new(vec![Series::new("datetime".into(), Vec::<String>::new()).into()])
                .unwrap();
        let err = signal_from_dataframe(&df).unwrap_err();
        assert!(err.to_string().contains("instrument"), "{err}");
    }
}
