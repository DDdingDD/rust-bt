//! benchmark 加载与校验（架构 §4.2）。
//!
//! 一个文件可含多个基准指数，全部加载；`gen_report(name)` 时按映射表选定。
//! 指数代码不按股票 int 编码（如 CSI932000），保持字符串原样存储。

use std::collections::HashMap;
use std::path::Path;

use chrono::NaiveDate;
use polars::prelude::*;

use crate::data::calendar::parse_date;
use crate::error::{BtError, Result};

/// benchmark 存储：原始行（覆盖校验在 gen_report 选定基准后进行）。
pub struct BenchmarkStore {
    pub dates: Vec<NaiveDate>,
    pub instruments: Vec<String>,
    pub values: Vec<f64>,
}

impl BenchmarkStore {
    /// 加载并做结构校验：必需列、(datetime, instrument) 重复。
    pub fn load(path: &Path) -> Result<Self> {
        let df = CsvReadOptions::default()
            .with_has_header(true)
            .try_into_reader_with_file_path(Some(path.to_path_buf()))?
            .finish()?;

        for name in ["datetime", "instrument", "benchmark"] {
            if df.column(name).is_err() {
                return Err(BtError::Validation(format!("benchmark 缺少必需列: {name}")));
            }
        }

        let raw_dates = df
            .column("datetime")?
            .cast(&DataType::String)?
            .str()?
            .iter()
            .map(|o| o.map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| BtError::Validation("benchmark datetime 存在缺失".into()))?;
        let instruments = df
            .column("instrument")?
            .cast(&DataType::String)?
            .str()?
            .iter()
            .map(|o| o.map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| BtError::Validation("benchmark instrument 存在缺失".into()))?;
        let values = df
            .column("benchmark")?
            .cast(&DataType::Float64)?
            .f64()?
            .iter()
            .map(|o| o.unwrap_or(f64::NAN))
            .collect::<Vec<_>>();

        let mut dates = Vec::with_capacity(raw_dates.len());
        for s in &raw_dates {
            dates.push(parse_date(s)?);
        }

        // (datetime, instrument) 重复 -> 报错
        let mut seen = std::collections::HashSet::with_capacity(dates.len());
        for (d, inst) in dates.iter().zip(&instruments) {
            if !seen.insert((*d, inst.as_str())) {
                return Err(BtError::Validation(format!(
                    "benchmark 存在重复键 (datetime={d}, instrument={inst})"
                )));
            }
        }

        Ok(Self {
            dates,
            instruments,
            values,
        })
    }

    /// 提取某指数代码的 日期 -> 收益率 序列（含非交易日行，调用方按日历过滤）。
    pub fn series_for(&self, instrument: &str) -> HashMap<NaiveDate, f64> {
        self.dates
            .iter()
            .zip(&self.instruments)
            .zip(&self.values)
            .filter(|((_, i), _)| i.as_str() == instrument)
            .map(|((d, _), v)| (*d, *v))
            .collect()
    }
}
