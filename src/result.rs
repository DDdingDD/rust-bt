//! BTResult（回测结果，架构 §4.9）：逐日账户快照 + 持仓历史 + 成交记录的导出与报表生成。

use std::ops::Range;
use std::time::Duration;

use chrono::NaiveDate;

use crate::account::{DailyRecord, HistPositionRow};
use crate::data::benchmark::BenchmarkStore;
use crate::data::calendar::TradingCalendar;
use crate::error::{BtError, Result};
use crate::order::TradeRecord;
use crate::report::Report;
use crate::types::{format_instrument, BenchmarkName, DayIdx, ExcessMethod};

/// 回测结果。
pub struct BTResult {
    daily: Vec<DailyRecord>,
    hist_positions: Vec<HistPositionRow>,
    trades: Vec<TradeRecord>,
    calendar: TradingCalendar,
    range: Range<DayIdx>,
    benchmark: Option<BenchmarkStore>,
    initial_cash: f64,
    /// run() 墙钟耗时（含启动校验与结果装配，不含 BTData 加载；元数据，不进导出文件）
    elapsed: Duration,
}

impl BTResult {
    pub(crate) fn assemble(
        daily: Vec<DailyRecord>,
        hist_positions: Vec<HistPositionRow>,
        trades: Vec<TradeRecord>,
        calendar: TradingCalendar,
        range: Range<DayIdx>,
        benchmark: Option<BenchmarkStore>,
        initial_cash: f64,
    ) -> Self {
        Self {
            daily,
            hist_positions,
            trades,
            calendar,
            range,
            benchmark,
            initial_cash,
            elapsed: Duration::ZERO,
        }
    }

    /// 逐日账户记录（测试与校验用）。
    pub fn daily(&self) -> &[DailyRecord] {
        &self.daily
    }

    /// 逐日持仓快照（测试与校验用）。
    pub fn hist_positions(&self) -> &[HistPositionRow] {
        &self.hist_positions
    }

    /// 成交记录（测试与校验用）。
    pub fn trades(&self) -> &[TradeRecord] {
        &self.trades
    }

    /// run() 墙钟耗时（进度条关闭时同样记录）。
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub(crate) fn set_elapsed(&mut self, elapsed: Duration) {
        self.elapsed = elapsed;
    }

    fn fmt_date(&self, day: DayIdx) -> String {
        self.calendar.date(day).format("%Y-%m-%d").to_string()
    }

    /// 导出历史持仓（规范"数据文件格式--hist_position.csv"）。
    /// weight = 该持仓市值 / 当日总资产（含成本口径），导出时计算；不输出 CASH 行。
    pub fn export_hist_position(&self, path: &str) -> Result<()> {
        let account_of_day: std::collections::HashMap<DayIdx, f64> =
            self.daily.iter().map(|d| (d.day, d.account)).collect();

        let mut datetime = Vec::with_capacity(self.hist_positions.len());
        let mut instrument = Vec::with_capacity(self.hist_positions.len());
        let mut volume = Vec::with_capacity(self.hist_positions.len());
        let mut cost_price = Vec::with_capacity(self.hist_positions.len());
        let mut price = Vec::with_capacity(self.hist_positions.len());
        let mut weight = Vec::with_capacity(self.hist_positions.len());
        let mut count_day = Vec::with_capacity(self.hist_positions.len());

        for r in &self.hist_positions {
            datetime.push(self.fmt_date(r.day));
            instrument.push(format_instrument(r.code)?);
            volume.push(r.volume);
            cost_price.push(r.cost_price);
            price.push(r.price);
            let account = account_of_day[&r.day];
            weight.push(if account != 0.0 {
                r.volume * r.price / account
            } else {
                0.0
            });
            count_day.push(r.count_day);
        }

        crate::report::write_csv(
            path,
            vec![
                ("datetime", Col::Str(datetime)),
                ("instrument", Col::Str(instrument)),
                ("volume", Col::F64(volume)),
                ("cost_price", Col::F64(cost_price)),
                ("price", Col::F64(price)),
                ("weight", Col::F64(weight)),
                ("count_day", Col::U32(count_day)),
            ],
        )
    }

    /// 导出成交记录（规范"数据文件格式--trades.csv"）：逐订单一行，含未成交订单。
    pub fn export_trades(&self, path: &str) -> Result<()> {
        let mut datetime = Vec::with_capacity(self.trades.len());
        let mut instrument = Vec::with_capacity(self.trades.len());
        let mut side = Vec::with_capacity(self.trades.len());
        let mut volume = Vec::with_capacity(self.trades.len());
        let mut price = Vec::with_capacity(self.trades.len());
        let mut deal_volume = Vec::with_capacity(self.trades.len());
        let mut deal_price = Vec::with_capacity(self.trades.len());
        let mut deal_cost = Vec::with_capacity(self.trades.len());

        for t in &self.trades {
            datetime.push(self.fmt_date(t.day));
            instrument.push(format_instrument(t.stock)?);
            side.push(t.side.as_str().to_owned());
            volume.push(t.volume);
            price.push(t.price);
            deal_volume.push(t.deal_volume);
            deal_price.push(t.deal_price);
            deal_cost.push(t.deal_cost);
        }

        crate::report::write_csv(
            path,
            vec![
                ("datetime", Col::Str(datetime)),
                ("instrument", Col::Str(instrument)),
                ("side", Col::Str(side)),
                ("volume", Col::F64(volume)),
                ("price", Col::F64(price)),
                ("deal_volume", Col::F64(deal_volume)),
                ("deal_price", Col::F64(deal_price)),
                ("deal_cost", Col::F64(deal_cost)),
            ],
        )
    }

    /// 生成报告：基准名 -> 映射表（不在表内 Err）-> 覆盖校验 -> 指标计算。
    pub fn gen_report(&self, benchmark: &str, excess_method: &str) -> Result<Report> {
        let name = BenchmarkName::from_name(benchmark).ok_or_else(|| {
            BtError::InvalidParam(format!("未知基准名称: {benchmark}（不在映射表）"))
        })?;
        let method = ExcessMethod::parse(excess_method)?;
        let store = self.benchmark.as_ref().ok_or_else(|| {
            BtError::BenchmarkCoverage("BTData 未加载 benchmark 数据".into())
        })?;
        let series = store.series_for(name.instrument());

        // 覆盖校验：基准必须覆盖回测区间全部交易日（日历外行天然不参与）；
        // 区间内交易日的 benchmark 值须有限（缺失/NaN/inf 按数据校验报错）。
        let mut bench_returns = Vec::with_capacity(self.range.len());
        for day in self.range.clone() {
            let date = self.calendar.date(day);
            match series.get(&date) {
                Some(v) if v.is_finite() => bench_returns.push(*v),
                Some(v) => {
                    return Err(BtError::Validation(format!(
                        "基准 {}（{}）在交易日 {date} 的 benchmark 值非法（缺失/NaN/inf）：{v}",
                        benchmark,
                        name.instrument()
                    )))
                }
                None => {
                    return Err(BtError::BenchmarkCoverage(format!(
                        "基准 {}（{}）缺失交易日 {date} 的数据",
                        benchmark,
                        name.instrument()
                    )))
                }
            }
        }

        Ok(Report::build(
            &self.daily,
            &bench_returns,
            self.daily_dates(),
            self.initial_cash,
            method,
        ))
    }

    /// 等价于 `gen_report("zz1000", "arithmetic")`。
    pub fn gen_report_default(&self) -> Result<Report> {
        self.gen_report("zz1000", "arithmetic")
    }

    fn daily_dates(&self) -> Vec<NaiveDate> {
        self.daily.iter().map(|d| self.calendar.date(d.day)).collect()
    }
}

/// 导出列（report::write_csv 的输入）。
pub enum Col {
    Str(Vec<String>),
    F64(Vec<f64>),
    U32(Vec<u32>),
}
