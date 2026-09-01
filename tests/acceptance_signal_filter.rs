//! 合成用例：信号日全部被过滤后不应导致次日清仓。
//!
//! 覆盖 `Backtest::run` 启动期过滤 + `api::signal_from_pairs` 全无效对。

mod common;

use common::*;
use rust_bt::{api, DataPaths, DataSource, DealPrice, ExchangeParams, ExcessMethod, StrategySpec};
use std::collections::BTreeMap;
use tempfile::TempDir;
use chrono::NaiveDate;

const D: [&str; 5] = [
    "2026-01-05",
    "2026-01-06",
    "2026-01-07",
    "2026-01-08",
    "2026-01-09",
];

#[test]
fn empty_signal_day_after_filtering_holds_positions() {
    // 行情：A/B/C 三只股票，d0..d4 正常交易
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    for d in D {
        for inst in ["SH600001", "SH600002", "SZ000001"] {
            bars.push(Bar::new(d, inst));
        }
    }
    write_stock_bar(dir.path(), &bars);
    write_bench(dir.path(), &bench_rows(&D, 0.001));

    // pred：d0 有 A/B/C 信号，d2 全部指向无行情的 SH699999（合法但无行情），
    // d0 信号让 d1 建仓 A/B；若 d2 被误当作“有信号日”，d3 会清仓全部持仓。
    write_pred(
        dir.path(),
        &[
            (D[0], "SH600001", 3.0),
            (D[0], "SH600002", 2.0),
            (D[0], "SZ000001", 1.0),
            (D[2], "SH699999", 3.0), // 全部过滤
        ],
    );

    let r = run_bt(&dir, &Params::default());

    // d1 建仓 A/B 各 5000 股；d2/d3 无有效信号 -> 持仓不动；d4 仍持有
    assert_eq!(r.trades().len(), 2);
    check_daily(
        &r,
        &[
            (100_000.0, 0.0, 100_000.0),
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
        ],
    );
}

#[test]
fn signal_from_pairs_all_invalid_drops_day() {
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    for d in D {
        for inst in ["SH600001", "SH600002", "SZ000001"] {
            bars.push(Bar::new(d, inst));
        }
    }
    write_stock_bar(dir.path(), &bars);
    write_bench(dir.path(), &bench_rows(&D, 0.001));
    write_pred(
        dir.path(),
        &[
            (D[0], "SH600001", 3.0),
            (D[0], "SH600002", 2.0),
            (D[0], "SZ000001", 1.0),
        ],
    );

    let mut days = BTreeMap::new();
    days.insert(
        NaiveDate::parse_from_str(D[2], "%Y-%m-%d").unwrap(),
        vec![("SH699999".to_string(), 1.0)], // 无法解析或后续过滤
    );
    days.insert(
        NaiveDate::parse_from_str(D[0], "%Y-%m-%d").unwrap(),
        vec![
            ("SH600001".to_string(), 3.0),
            ("SH600002".to_string(), 2.0),
            ("SZ000001".to_string(), 1.0),
        ],
    );
    let signal = api::signal_from_pairs(days).unwrap();

    let params = rust_bt::BtParams {
        data: DataSource::Paths(DataPaths {
            stock_bar: dir.path().join("stock_bar.csv").to_str().unwrap().into(),
            benchmark: dir.path().join("benchmark.csv").to_str().unwrap().into(),
            wap: None,
        }),
        start_date: "2026-01-05".into(),
        end_date: "2026-01-10".into(),
        initial_cash: 100_000.0,
        strategy: StrategySpec::topk_dropout(2, 1),
        exchange: ExchangeParams {
            deal_price: DealPrice::Open,
            open_cost: 0.0,
            close_cost: 0.0,
            min_cost: 0.0,
            fixed_slippage: 0.0,
            min_slippage_ratio: 0.0,
            volume_threshold: None,
            limit_threshold: Some(0.0985),
        },
        benchmark_name: rust_bt::BenchmarkName::Zz1000,
        excess_method: ExcessMethod::Arithmetic,
        progress: false,
    };
    let output = api::run(params, &signal).unwrap();

    // 与 CSV 路径一致：d1 建仓后一直持有
    check_daily(
        &output.result,
        &[
            (100_000.0, 0.0, 100_000.0),
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
        ],
    );
}
