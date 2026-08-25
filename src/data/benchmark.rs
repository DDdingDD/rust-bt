//! benchmark 加载与校验（架构 §4.2）。
//!
//! 一个文件可含多个基准指数，全部加载；`gen_report(name)` 时按映射表选定。
//! 指数代码不按股票 int 编码（如 CSI932000），保持字符串原样存储。

use std::collections::HashMap;
use std::path::Path;

use chrono::NaiveDate;
use polars::prelude::*;

use crate::data::calendar::parse_date;
use crate::data::{date_strings, read_dataframe};
use crate::error::{BtError, Result};

/// benchmark 存储：原始行（覆盖校验在 gen_report 选定基准后进行）。
pub struct BenchmarkStore {
    pub dates: Vec<NaiveDate>,
    pub instruments: Vec<String>,
    pub values: Vec<f64>,
}

impl BenchmarkStore {
    /// 加载并做结构校验（CSV 或 parquet，按扩展名识别）：必需列、(datetime, instrument) 重复。
    pub fn load(path: &Path) -> Result<Self> {
        let df = read_dataframe(path)?;

        for name in ["datetime", "instrument", "benchmark"] {
            if df.column(name).is_err() {
                return Err(BtError::Validation(format!("benchmark 缺少必需列: {name}")));
            }
        }

        let raw_dates = date_strings(&df, "datetime")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// parquet 加载：datetime 为 Datetime 类型（pandas datetime64[ns] 常见形态），
    /// 时间部分截断到日后与 CSV 口径一致。
    #[test]
    fn load_parquet_datetime_typed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("benchmark.parquet");

        let ns = |d: &str| {
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(12, 30, 0) // 带时间，验证截断
                .unwrap()
                .and_utc()
                .timestamp_nanos_opt()
                .unwrap()
        };
        let datetime = Series::new(
            "datetime".into(),
            vec![ns("2026-01-05"), ns("2026-01-05"), ns("2026-01-06")],
        )
        .cast(&DataType::Datetime(TimeUnit::Nanoseconds, None))
        .unwrap();
        let mut df = DataFrame::new(vec![
            datetime.into(),
            Series::new("instrument".into(), vec!["SH000852", "SH000300", "SH000852"]).into(),
            Series::new("benchmark".into(), vec![0.001, 0.002, -0.003]).into(),
        ])
        .unwrap();
        let file = std::fs::File::create(&path).unwrap();
        ParquetWriter::new(file).finish(&mut df).unwrap();

        let store = BenchmarkStore::load(&path).unwrap();
        assert_eq!(
            store.instruments,
            vec!["SH000852", "SH000300", "SH000852"]
        );
        assert_eq!(store.values, vec![0.001, 0.002, -0.003]);
        let series = store.series_for("SH000852");
        assert_eq!(series.len(), 2);
        assert_eq!(
            series.get(&NaiveDate::from_ymd_opt(2026, 1, 5).unwrap()),
            Some(&0.001)
        );
        assert_eq!(
            series.get(&NaiveDate::from_ymd_opt(2026, 1, 6).unwrap()),
            Some(&-0.003)
        );
    }
}
