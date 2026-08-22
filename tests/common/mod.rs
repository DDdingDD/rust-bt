//! 验收测试共用构件：合成 CSV 生成与回测运行辅助。
//!
//! 合成数据约定（架构 §8）：3~5 只股票、约 10 个交易日、价格取整数 / 有限小数，
//! 消除浮点歧义，逐笔成交与逐日账户均可手算对拍。

// 各集成测试 crate 只使用部分辅助函数，未用项不告警。
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use rust_bt::*;
use tempfile::TempDir;

/// 合成 stock_bar 行。`None` 表示该字段缺失（CSV 空值）。
#[derive(Clone)]
pub struct Bar {
    pub date: &'static str,
    pub inst: &'static str,
    pub open: f64,
    pub close: Option<f64>,
    pub volume: f64,
    pub factor: f64,
    pub high_limit: Option<f64>,
    pub low_limit: Option<f64>,
    pub pre_close: Option<f64>,
    pub paused: u8,
    pub is_st: u8,
    pub vwap: Option<f64>,
}

impl Bar {
    /// 常规行：open=close=10，pre_close=10，板 11/9，factor=1，放量，非停牌非 ST。
    pub fn new(date: &'static str, inst: &'static str) -> Self {
        Self {
            date,
            inst,
            open: 10.0,
            close: Some(10.0),
            volume: 1_000_000.0,
            factor: 1.0,
            high_limit: Some(11.0),
            low_limit: Some(9.0),
            pre_close: Some(10.0),
            paused: 0,
            is_st: 0,
            vwap: Some(10.0),
        }
    }
}

fn fnum(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn fopt(v: Option<f64>) -> String {
    v.map(fnum).unwrap_or_default()
}

/// 写 stock_bar.csv（low/high/money/avg 由 open/close 推导，避免无关字段干扰）。
pub fn write_stock_bar(dir: &Path, bars: &[Bar]) -> PathBuf {
    let mut s = String::from(
        "datetime,instrument,open,close,low,high,volume,money,factor,high_limit,low_limit,avg,pre_close,paused,is_st,vwap\n",
    );
    for b in bars {
        let close = b.close.unwrap_or(b.open);
        let low = b.open.min(close) - 1.0;
        let high = b.open.max(close) + 1.0;
        let money = close * b.volume;
        s.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            b.date,
            b.inst,
            fnum(b.open),
            fopt(b.close),
            fnum(low),
            fnum(high),
            fnum(b.volume),
            fnum(money),
            fnum(b.factor),
            fopt(b.high_limit),
            fopt(b.low_limit),
            fopt(b.vwap), // avg 与 vwap 同值
            fopt(b.pre_close),
            b.paused,
            b.is_st,
            fopt(b.vwap)
        ));
    }
    let p = dir.join("stock_bar.csv");
    fs::write(&p, s).unwrap();
    p
}

/// 写 pred.csv（ret 列固定 0，仅验证隔离）。
pub fn write_pred(dir: &Path, rows: &[(&str, &str, f64)]) -> PathBuf {
    let mut s = String::from("datetime,instrument,score,ret\n");
    for (d, i, score) in rows {
        s.push_str(&format!("{d},{i},{score},0\n"));
    }
    let p = dir.join("pred.csv");
    fs::write(&p, s).unwrap();
    p
}

/// 写 benchmark.csv。
pub fn write_bench(dir: &Path, rows: &[(&str, &str, f64)]) -> PathBuf {
    let mut s = String::from("datetime,instrument,benchmark\n");
    for (d, i, v) in rows {
        s.push_str(&format!("{d},{i},{v}\n"));
    }
    let p = dir.join("benchmark.csv");
    fs::write(&p, s).unwrap();
    p
}

/// 为给定日期批量生成 zz1000（SH000852）基准行。
pub fn bench_rows<'a>(dates: &[&'a str], value: f64) -> Vec<(&'a str, &'static str, f64)> {
    dates.iter().map(|d| (*d, "SH000852", value)).collect()
}

/// 回测参数（默认零成本零滑点，便于手算）。
pub struct Params {
    pub cash: f64,
    pub top_n: usize,
    pub drop_n: usize,
    pub deal_price: String,
    pub open_cost: f64,
    pub close_cost: f64,
    pub min_cost: f64,
    pub fixed_slippage: f64,
    pub min_slippage_ratio: f64,
    pub volume_threshold: Option<f64>,
    pub limit_threshold: Option<f64>,
    pub start: String,
    pub end: String,
    pub with_benchmark: bool,
    pub progress: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            cash: 100_000.0,
            top_n: 2,
            drop_n: 1,
            deal_price: "open".into(),
            open_cost: 0.0,
            close_cost: 0.0,
            min_cost: 0.0,
            fixed_slippage: 0.0,
            min_slippage_ratio: 0.0,
            volume_threshold: None,
            limit_threshold: Some(0.0985),
            start: "2026-01-05".into(),
            end: "2026-01-10".into(),
            with_benchmark: false,
            progress: false,
        }
    }
}

/// 完整跑通一次回测（TopkDropout 策略）。
pub fn run_bt(dir: &TempDir, params: &Params) -> BTResult {
    let strategy: Box<dyn Strategy> =
        Box::new(TopkDropoutStrategy::new(params.top_n, params.drop_n));
    run_bt_with(dir, params, strategy).unwrap()
}

/// 以自定义策略跑通一次回测（保留 Err，供引擎级行为断言）。
pub fn run_bt_with(
    dir: &TempDir,
    params: &Params,
    strategy: Box<dyn Strategy>,
) -> rust_bt::Result<BTResult> {
    let signal = load_signal(dir.path().join("pred.csv").to_str().unwrap()).unwrap();
    let data = BTData::new()
        .load_stock_bar(dir.path().join("stock_bar.csv").to_str().unwrap())
        .unwrap();
    let data = if params.with_benchmark {
        data.load_benchmark(dir.path().join("benchmark.csv").to_str().unwrap())
            .unwrap()
    } else {
        data
    };
    let data = data.build().unwrap();
    let account = Account::new(params.cash);
    let exchange = Exchange::new(
        &params.deal_price,
        params.open_cost,
        params.close_cost,
        params.min_cost,
        params.fixed_slippage,
        params.min_slippage_ratio,
        params.volume_threshold,
        params.limit_threshold,
    )
    .unwrap();
    let mut bt = Backtest::new(data, account, exchange, strategy).with_progress(params.progress);
    bt.run(&signal, &params.start, &params.end)
}

/// 浮点对拍（合成用例数值均可精确表示，容差仅防累乘顺序差异）。
pub fn assert_f64(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= 1e-9 * expected.abs().max(1.0),
        "{what}: 实际 {actual}，期望 {expected}"
    );
}

/// 逐笔成交对拍。
#[allow(clippy::too_many_arguments)]
pub fn check_trade(
    t: &TradeRecord,
    day: DayIdx,
    inst: &str,
    side: Side,
    volume: f64,
    price: f64,
    deal_volume: f64,
    deal_price: f64,
    deal_cost: f64,
) {
    assert_eq!(t.day, day, "trade day");
    assert_eq!(t.stock, parse_instrument(inst).unwrap(), "trade instrument");
    assert_eq!(t.side, side, "trade side");
    assert_f64(t.volume, volume, "trade volume");
    assert_f64(t.price, price, "trade price");
    assert_f64(t.deal_volume, deal_volume, "trade deal_volume");
    assert_f64(t.deal_price, deal_price, "trade deal_price");
    assert_f64(t.deal_cost, deal_cost, "trade deal_cost");
}

/// 逐日账户对拍：(account, value, cash)。
pub fn check_daily(result: &BTResult, expected: &[(f64, f64, f64)]) {
    let daily = result.daily();
    assert_eq!(daily.len(), expected.len(), "daily 天数");
    for (i, (d, e)) in daily.iter().zip(expected).enumerate() {
        assert_f64(d.account, e.0, &format!("daily[{i}].account"));
        assert_f64(d.value, e.1, &format!("daily[{i}].value"));
        assert_f64(d.cash, e.2, &format!("daily[{i}].cash"));
    }
}

/// 不变量：每日持仓只数 <= top_n。
pub fn assert_positions_cap(result: &BTResult, top_n: usize) {
    let mut per_day: std::collections::BTreeMap<DayIdx, usize> = Default::default();
    for r in result.hist_positions() {
        *per_day.entry(r.day).or_default() += 1;
    }
    for (day, n) in per_day {
        assert!(n <= top_n, "day {day} 持仓 {n} 只超过 top_n={top_n}");
    }
}

/// 某股票某日的持仓快照行。
pub fn hist_row<'a>(result: &'a BTResult, day: DayIdx, inst: &str) -> Option<&'a HistPositionRow> {
    let code = parse_instrument(inst).unwrap();
    result
        .hist_positions()
        .iter()
        .find(|r| r.day == day && r.code == code)
}
