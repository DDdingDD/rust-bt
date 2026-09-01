//! wap 数据加载（架构 §4.2）：时段 VWAP/TWAP 方向价格与可成交量。
//!
//! `deal_price = vwapN / twapN` 时经 `BTData::load_wap` 载入，在
//! `DailyMarketStore::build` 与 stock_bar 按 (date, code) 归并联接。同一份数据
//! 同时保留 vwap 与 twap 两种价格族的方向列，vwapN / twapN 切换无需重载。
//!
//! 列语义（以 wap_11 为例，规范"数据文件格式--wap 数据"）：
//! - `wap_11_vwap_buy` / `wap_11_twap_buy`：时段内可买入价（剔除涨停 tick）
//! - `wap_11_vwap_sell` / `wap_11_twap_sell`：时段内可卖出价（剔除跌停 tick）
//! - `wap_11_buy_volume` / `wap_11_sell_volume`：对应方向可成交量（剔除触板 tick）
//!
//! 价格 0 / 缺失 / ≤ 0 表示该方向时段内无可成交 tick（**非错误**），联接后表现为
//! 方向价 NaN + 量上限 0，即该方向不可交易；中性列（`wap_N_vwap` / `wap_N_twap`）
//! 无消费方（委托定价用 pre_close，涨跌停判定用方向价），不加载。

use std::path::Path;

use chrono::NaiveDate;

use crate::data::calendar::parse_date;
use crate::data::date_strings;
use crate::data::read_dataframe_columns;
use crate::data::stock_bar::{f64_column, str_column};
use crate::error::{BtError, Result};
use crate::types::{parse_instrument, Code, WAP_WINDOW_MAX};

/// wap 存储：行按 (date, code) 升序的 SoA（date 为 NaiveDate，联接时经 stock_bar
/// 交易日历对齐；wap 中不在 stock_bar 日历内的日期自然不参与联接）。
#[derive(Debug)]
pub struct WapStore {
    /// 时段号 1..=11（联接时须与 deal_price 的时段一致）
    pub window: u8,
    pub dates: Vec<NaiveDate>,
    pub codes: Vec<Code>,
    pub vwap_buy: Vec<f64>,
    pub vwap_sell: Vec<f64>,
    pub twap_buy: Vec<f64>,
    pub twap_sell: Vec<f64>,
    pub buy_volume: Vec<f64>,
    pub sell_volume: Vec<f64>,
}

impl WapStore {
    /// 加载并校验 wap 数据（CSV 或 parquet，按扩展名识别；parquet 走列投影）。
    ///
    /// 校验：时段号 1..=11、必需列存在、(datetime, instrument) 无重复、
    /// volume 非负。价格 / volume 缺失（NaN）与价格 ≤ 0 不报错，语义为该方向不可成交。
    pub fn load(path: &Path, window: u8) -> Result<Self> {
        if !(1..=WAP_WINDOW_MAX).contains(&window) {
            return Err(BtError::InvalidParam(format!(
                "wap 时段号须在 1..={WAP_WINDOW_MAX}，收到: {window}"
            )));
        }
        let columns = required_columns(window);
        let df = read_dataframe_columns(path, &columns).map_err(|e| {
            BtError::Validation(format!(
                "读取 wap 数据失败（检查文件与列 wap_{window}_* 是否存在）: {e}"
            ))
        })?;
        for name in &columns {
            if df.column(name).is_err() {
                return Err(BtError::Validation(format!(
                    "wap 数据（时段 {window}）缺少必需列: {name}"
                )));
            }
        }

        let n = df.height();
        let raw_dates = date_strings(&df, "datetime")?;
        let raw_instruments = str_column(&df, "instrument")?;
        let mut vwap_buy = f64_column(&df, &format!("wap_{window}_vwap_buy"))?;
        let mut vwap_sell = f64_column(&df, &format!("wap_{window}_vwap_sell"))?;
        let mut twap_buy = f64_column(&df, &format!("wap_{window}_twap_buy"))?;
        let mut twap_sell = f64_column(&df, &format!("wap_{window}_twap_sell"))?;
        let mut buy_volume = f64_column(&df, &format!("wap_{window}_buy_volume"))?;
        let mut sell_volume = f64_column(&df, &format!("wap_{window}_sell_volume"))?;

        let mut dates: Vec<NaiveDate> = Vec::with_capacity(n);
        for s in &raw_dates {
            dates.push(parse_date(s)?);
        }
        let mut codes: Vec<Code> = Vec::with_capacity(n);
        for s in &raw_instruments {
            codes.push(parse_instrument(s)?);
        }

        // 按 (date, code) 排序（与 stock_bar 的 (DayIdx, Code) 序一致，供归并联接）
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_unstable_by_key(|i| (dates[*i], codes[*i]));
        for w in order.windows(2) {
            if (dates[w[0]], codes[w[0]]) == (dates[w[1]], codes[w[1]]) {
                return Err(BtError::Validation(format!(
                    "wap 数据存在重复键 (datetime={}, instrument={})",
                    raw_dates[w[0]], raw_instruments[w[0]]
                )));
            }
        }
        let permute = |v: &mut Vec<f64>| {
            let old = std::mem::take(v);
            *v = order.iter().map(|i| old[*i]).collect();
        };
        permute(&mut vwap_buy);
        permute(&mut vwap_sell);
        permute(&mut twap_buy);
        permute(&mut twap_sell);
        permute(&mut buy_volume);
        permute(&mut sell_volume);
        let old_dates = std::mem::take(&mut dates);
        let dates: Vec<NaiveDate> = order.iter().map(|i| old_dates[*i]).collect();
        let old_codes = std::mem::take(&mut codes);
        let codes: Vec<Code> = order.iter().map(|i| old_codes[*i]).collect();

        // 行级硬校验：volume 非负且有限（缺失 NaN / Inf 放行为 0 量，价格缺失 / <= 0 放行
        // 为该方向不可成交）
        for (i, &c) in codes.iter().enumerate() {
            for (name, col) in [
                ("buy_volume", &buy_volume),
                ("sell_volume", &sell_volume),
            ] {
                let v = col[i];
                if !v.is_nan() && (!v.is_finite() || v < 0.0) {
                    return Err(BtError::Validation(format!(
                        "wap 数据行 {i}（code={c}）{name} = {v} 非法（负/非有限）"
                    )));
                }
            }
        }

        Ok(Self {
            window,
            dates,
            codes,
            vwap_buy,
            vwap_sell,
            twap_buy,
            twap_sell,
            buy_volume,
            sell_volume,
        })
    }
}

/// 时段 `window` 的必需列（datetime / instrument + 两种价格族的方向价与方向量）。
fn required_columns(window: u8) -> Vec<String> {
    [
        "datetime".to_string(),
        "instrument".to_string(),
        format!("wap_{window}_vwap_buy"),
        format!("wap_{window}_vwap_sell"),
        format!("wap_{window}_twap_buy"),
        format!("wap_{window}_twap_sell"),
        format!("wap_{window}_buy_volume"),
        format!("wap_{window}_sell_volume"),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use polars::prelude::*;

    use super::*;

    fn write_wap_csv(path: &Path, window: u8, body: &str) {
        let s = format!(
            "datetime,instrument,wap_{w}_vwap_buy,wap_{w}_vwap_sell,wap_{w}_twap_buy,wap_{w}_twap_sell,wap_{w}_buy_volume,wap_{w}_sell_volume\n{body}",
            w = window
        );
        std::fs::write(path, s).unwrap();
    }

    #[test]
    fn load_csv_sorted_and_validated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wap.csv");
        // 乱序输入：SZ000001(code=1) < SH600001(code=600001)
        write_wap_csv(
            &path,
            11,
            "2026-01-05,SH600001,10.5,9.5,10.4,9.6,100,120\n\
             2026-01-05,SZ000001,20.5,19.5,20.4,19.6,200,220\n\
             2026-01-06,SH600001,0,9.8,0,9.7,0,130\n",
        );
        let store = WapStore::load(&path, 11).unwrap();
        assert_eq!(store.window, 11);
        assert_eq!(store.codes, vec![1, 600_001, 600_001]);
        assert_eq!(store.vwap_buy, vec![20.5, 10.5, 0.0]);
        assert_eq!(store.vwap_sell, vec![19.5, 9.5, 9.8]);
        assert_eq!(store.twap_buy, vec![20.4, 10.4, 0.0]);
        assert_eq!(store.twap_sell, vec![19.6, 9.6, 9.7]);
        assert_eq!(store.buy_volume, vec![200.0, 100.0, 0.0]);
        assert_eq!(store.sell_volume, vec![220.0, 120.0, 130.0]);
        assert_eq!(store.dates.len(), 3);
    }

    #[test]
    fn load_parquet_typed_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wap.parquet");
        let datetime = Series::new(
            "datetime".into(),
            vec!["2026-01-05", "2026-01-05", "2026-01-06"],
        )
        .cast(&DataType::Date)
        .unwrap();
        // parquet 实际数据为 f32 类型化列，验证 cast 路径
        let f32col = |name: &str, vals: Vec<f32>| Series::new(name.into(), vals);
        let mut df = DataFrame::new(vec![
            datetime.into(),
            Series::new(
                "instrument".into(),
                vec!["SH600001", "SZ000001", "SH600001"],
            )
            .into(),
            f32col("wap_5_vwap_buy", vec![10.5, 20.5, 11.0]).into(),
            f32col("wap_5_vwap_sell", vec![9.5, 19.5, 10.8]).into(),
            f32col("wap_5_twap_buy", vec![10.4, 20.4, 11.0]).into(),
            f32col("wap_5_twap_sell", vec![9.6, 19.6, 10.9]).into(),
            f32col("wap_5_buy_volume", vec![100.0, 200.0, 110.0]).into(),
            f32col("wap_5_sell_volume", vec![120.0, 220.0, 130.0]).into(),
        ])
        .unwrap();
        let file = std::fs::File::create(&path).unwrap();
        ParquetWriter::new(file).finish(&mut df).unwrap();

        let store = WapStore::load(&path, 5).unwrap();
        assert_eq!(store.codes, vec![1, 600_001, 600_001]);
        assert_eq!(store.vwap_buy, vec![20.5, 10.5, 11.0]);
        assert_eq!(store.sell_volume, vec![220.0, 120.0, 130.0]);
    }

    #[test]
    fn invalid_window_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wap.csv");
        write_wap_csv(&path, 11, "");
        assert!(WapStore::load(&path, 0).is_err());
        assert!(WapStore::load(&path, 12).is_err());
    }

    #[test]
    fn duplicate_key_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wap.csv");
        write_wap_csv(
            &path,
            11,
            "2026-01-05,SH600001,10.5,9.5,10.4,9.6,100,120\n\
             2026-01-05,SH600001,10.6,9.6,10.5,9.7,101,121\n",
        );
        let err = WapStore::load(&path, 11).unwrap_err().to_string();
        assert!(err.contains("重复键"), "{err}");
    }

    #[test]
    fn missing_required_column_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wap.csv");
        // 只有时段 11 的列，却按时段 5 加载 -> 缺列
        write_wap_csv(&path, 11, "2026-01-05,SH600001,10.5,9.5,10.4,9.6,100,120\n");
        let err = WapStore::load(&path, 5).unwrap_err().to_string();
        assert!(err.contains("wap_5"), "{err}");
    }

    #[test]
    fn negative_volume_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wap.csv");
        write_wap_csv(
            &path,
            11,
            "2026-01-05,SH600001,10.5,9.5,10.4,9.6,-1,120\n",
        );
        let err = WapStore::load(&path, 11).unwrap_err().to_string();
        assert!(err.contains("buy_volume"), "{err}");
    }

    #[test]
    fn rejects_infinite_volume() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wap.csv");
        write_wap_csv(
            &path,
            11,
            "2026-01-05,SH600001,10.5,9.5,10.4,9.6,inf,120\n",
        );
        let err = WapStore::load(&path, 11).unwrap_err().to_string();
        assert!(err.contains("buy_volume") && err.contains("有限"), "{err}");
    }
}
