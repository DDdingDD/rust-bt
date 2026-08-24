//! 嵌入 API（api::run）集成测试：与组件层 Facade 口径对拍 + 内存信号 + 自定义策略 + 导出。
//!
//! 对拍基准：同一合成场景（与 acceptance_basic 的 build_and_rebalance 同构）
//! 分别经 `api::run`（内存信号）与组件层（pred.csv）运行，逐日账户与逐笔
//! 成交必须完全一致--两层共用同一撮合与估值路径。

mod common;

use std::collections::BTreeMap;

use chrono::NaiveDate;
use common::*;
use rust_bt::{api, BtParams, DealPrice, ExportNames, ExchangeParams, StrategySpec, TopkDropoutStrategy};
use tempfile::TempDir;

const D: [&str; 5] = [
    "2026-01-05",
    "2026-01-06",
    "2026-01-07",
    "2026-01-08",
    "2026-01-09",
];

fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// 与 acceptance_basic 相同的场景：3 只股票、5 个交易日、零成本零滑点。
fn setup(with_bench: bool) -> (TempDir, Params) {
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    for d in D {
        for inst in ["SH600001", "SH600002", "SZ000001"] {
            bars.push(Bar::new(d, inst));
        }
    }
    write_stock_bar(dir.path(), &bars);
    write_pred(
        dir.path(),
        &[
            (D[0], "SH600001", 3.0),
            (D[0], "SH600002", 2.0),
            (D[0], "SZ000001", 1.0),
            (D[2], "SH600001", 1.0),
            (D[2], "SH600002", 2.0),
            (D[2], "SZ000001", 3.0),
        ],
    );
    write_bench(dir.path(), &bench_rows(&D, 0.001));
    let params = Params {
        with_benchmark: with_bench,
        ..Default::default()
    };
    (dir, params)
}

/// 嵌入 API 参数（对齐 common::Params 默认：零成本零滑点，top_n=2，drop_n=1）。
fn api_params(dir: &TempDir, strategy: StrategySpec) -> BtParams {
    BtParams {
        stock_bar: dir.path().join("stock_bar.csv").to_str().unwrap().into(),
        benchmark: dir.path().join("benchmark.csv").to_str().unwrap().into(),
        start_date: "2026-01-05".into(),
        end_date: "2026-01-10".into(),
        initial_cash: 100_000.0,
        strategy,
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
        excess_method: rust_bt::ExcessMethod::Arithmetic,
        progress: false,
    }
}

/// 与 write_pred 相同信号的内存形态。
fn in_memory_signal() -> rust_bt::Signal {
    let mut days = BTreeMap::new();
    days.insert(
        date(D[0]),
        vec![
            ("SH600001".to_string(), 3.0),
            ("SH600002".to_string(), 2.0),
            ("SZ000001".to_string(), 1.0),
        ],
    );
    days.insert(
        date(D[2]),
        vec![
            ("SH600001".to_string(), 1.0),
            ("SH600002".to_string(), 2.0),
            ("SZ000001".to_string(), 3.0),
        ],
    );
    api::signal_from_pairs(days).unwrap()
}

/// 逐日账户逐字段对拍（DailyRecord 未派生 PartialEq，按字段比）。
fn assert_daily_same(a: &rust_bt::BTResult, b: &rust_bt::BTResult) {
    assert_eq!(a.daily().len(), b.daily().len(), "daily 天数");
    for (x, y) in a.daily().iter().zip(b.daily()) {
        assert_eq!(x.day, y.day);
        assert_f64(x.account, y.account, "account");
        assert_f64(x.value, y.value, "value");
        assert_f64(x.cash, y.cash, "cash");
        assert_f64(x.turnover_amount, y.turnover_amount, "turnover_amount");
        assert_f64(x.cost, y.cost, "cost");
    }
}

/// 逐笔成交逐字段对拍。
fn assert_trades_same(a: &rust_bt::BTResult, b: &rust_bt::BTResult) {
    assert_eq!(a.trades().len(), b.trades().len(), "trades 笔数");
    for (x, y) in a.trades().iter().zip(b.trades()) {
        assert_eq!(x.day, y.day);
        assert_eq!(x.stock, y.stock);
        assert_eq!(x.side, y.side);
        assert_f64(x.deal_volume, y.deal_volume, "deal_volume");
        assert_f64(x.deal_price, y.deal_price, "deal_price");
        assert_f64(x.deal_cost, y.deal_cost, "deal_cost");
    }
}

#[test]
fn in_memory_signal_run_matches_component_layer() {
    let (dir, params) = setup(true);
    let expected = run_bt(&dir, &params);

    let output = api::run(
        api_params(&dir, StrategySpec::topk_dropout(2, 1)),
        &in_memory_signal(),
    )
    .unwrap();

    assert_daily_same(&output.result, &expected);
    assert_trades_same(&output.result, &expected);
    assert_eq!(
        output.result.hist_positions().len(),
        expected.hist_positions().len(),
        "hist_positions 行数"
    );

    // 报告序列访问器与组件层 gen_report 一致
    let report = expected.gen_report("zz1000", "arithmetic").unwrap();
    assert_eq!(output.report.dates(), report.dates());
    assert_eq!(output.report.cum_with_cost(), report.cum_with_cost());
    assert_eq!(output.report.turnover(), report.turnover());
    assert_f64(
        output.report.derived.annualized_return,
        report.derived.annualized_return,
        "annualized_return",
    );
}

#[test]
fn custom_strategy_spec_matches_builtin_topk() {
    let (dir, _) = setup(true);

    let builtin = api::run(
        api_params(&dir, StrategySpec::topk_dropout(2, 1)),
        &in_memory_signal(),
    )
    .unwrap();
    let custom = api::run(
        api_params(
            &dir,
            StrategySpec::Custom(Box::new(TopkDropoutStrategy::new(2, 1))),
        ),
        &in_memory_signal(),
    )
    .unwrap();

    assert_daily_same(&custom.result, &builtin.result);
    assert_trades_same(&custom.result, &builtin.result);
}

#[test]
fn export_all_writes_four_artifacts() {
    let (dir, _) = setup(true);
    let out_dir = dir.path().join("out");

    let output = api::run(
        api_params(&dir, StrategySpec::topk_dropout(2, 1)),
        &in_memory_signal(),
    )
    .unwrap();
    output.export_all(&out_dir, &ExportNames::default()).unwrap();

    for name in [
        "hist_position.csv",
        "trades.csv",
        "report_data.csv",
        "report_plot.html",
    ] {
        let p = out_dir.join(name);
        let meta = std::fs::metadata(&p)
            .unwrap_or_else(|e| panic!("{name} 应存在: {e}"));
        assert!(meta.len() > 0, "{name} 不应为空文件");
    }
}

#[test]
fn signal_from_pairs_drops_invalid_and_errors_on_duplicate() {
    use rust_bt::SignalDay;

    // 不可解析 instrument 与 NaN score 丢弃（口径同 load_signal），合法行保留
    let day = SignalDay::from_pairs(vec![
        ("SH600001".to_string(), 1.0),
        ("BJ832000".to_string(), 2.0), // 北交所前缀，不在支持范围
        ("SH600002".to_string(), f64::NAN),
    ])
    .unwrap();
    assert_eq!(day.codes, vec![600001]);
    assert_eq!(day.scores, vec![1.0]);

    // 同日重复 instrument -> Err
    let err = SignalDay::from_pairs(vec![
        ("SH600001".to_string(), 1.0),
        ("SH600001".to_string(), 2.0),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("SH600001"), "{err}");
}
