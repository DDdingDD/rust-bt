//! 交易日历（架构 §4.2）：升序日期 + 日期 -> DayIdx 反查，区间对齐与校验。

use std::collections::HashMap;
use std::ops::Range;

use chrono::NaiveDate;

use crate::error::{BtError, Result};
use crate::types::DayIdx;

/// 解析 `YYYY-MM-DD` 日期字符串。
pub fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| BtError::Calendar(format!("日期须为 YYYY-MM-DD 格式，收到: {s}")))
}

/// 交易日历 = stock_bar.csv 中去重排序后的全部 `datetime`。
#[derive(Clone, Debug)]
pub struct TradingCalendar {
    dates: Vec<NaiveDate>,
    index: HashMap<NaiveDate, DayIdx>,
}

impl TradingCalendar {
    /// 由乱序（可含重复）日期构建：排序去重并建反查。
    pub fn from_dates(mut dates: Vec<NaiveDate>) -> Self {
        dates.sort_unstable();
        dates.dedup();
        let index = dates
            .iter()
            .enumerate()
            .map(|(i, d)| (*d, i as DayIdx))
            .collect();
        Self { dates, index }
    }

    /// 闭开区间 [start, end) 对齐：实际首日 = 区间内第一个交易日，
    /// 实际末日 = 区间内最后一个交易日（不含 end_date）。
    /// `start >= end` 或区间内无交易日 -> Err。
    pub fn align(&self, start: &str, end: &str) -> Result<Range<DayIdx>> {
        let start = parse_date(start)?;
        let end = parse_date(end)?;
        if start >= end {
            return Err(BtError::Calendar(format!(
                "回测区间非法：start_date({start}) >= end_date({end})"
            )));
        }
        let first = self.dates.partition_point(|d| *d < start) as DayIdx;
        let past_end = self.dates.partition_point(|d| *d < end) as DayIdx;
        if first >= past_end {
            return Err(BtError::Calendar(format!(
                "回测区间 [{start}, {end}) 内不含任何交易日"
            )));
        }
        Ok(first..past_end)
    }

    /// DayIdx -> 日期。
    pub fn date(&self, idx: DayIdx) -> NaiveDate {
        self.dates[idx as usize]
    }

    /// 日期 -> DayIdx。
    pub fn day_idx(&self, d: &NaiveDate) -> Option<DayIdx> {
        self.index.get(d).copied()
    }

    /// 是否为交易日。
    pub fn contains(&self, d: &NaiveDate) -> bool {
        self.index.contains_key(d)
    }

    /// 交易日总数。
    pub fn len(&self) -> usize {
        self.dates.len()
    }

    /// 是否为空日历。
    pub fn is_empty(&self) -> bool {
        self.dates.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal() -> TradingCalendar {
        TradingCalendar::from_dates(vec![
            parse_date("2026-01-06").unwrap(),
            parse_date("2026-01-05").unwrap(),
            parse_date("2026-01-08").unwrap(),
            parse_date("2026-01-07").unwrap(),
            parse_date("2026-01-06").unwrap(), // 重复
        ])
    }

    #[test]
    fn dedup_sort() {
        let c = cal();
        assert_eq!(c.len(), 4);
        assert_eq!(c.date(0), parse_date("2026-01-05").unwrap());
        assert_eq!(c.day_idx(&parse_date("2026-01-08").unwrap()), Some(3));
    }

    #[test]
    fn align_basic() {
        let c = cal();
        // 闭开区间：不含 end_date 当日
        assert_eq!(c.align("2026-01-05", "2026-01-08").unwrap(), 0..3);
        // 非交易日自动对齐到区间内最近交易日
        assert_eq!(c.align("2026-01-04", "2026-01-09").unwrap(), 0..4);
        assert_eq!(c.align("2026-01-06", "2026-01-08").unwrap(), 1..3);
        // 区间内无交易日
        assert!(c.align("2026-02-01", "2026-03-01").is_err());
        // start >= end
        assert!(c.align("2026-01-08", "2026-01-05").is_err());
        assert!(c.align("2026-01-05", "2026-01-05").is_err());
        // 非法日期格式
        assert!(c.align("2026/01/05", "2026-01-08").is_err());
    }
}
