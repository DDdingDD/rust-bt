//! 数据目录自动发现（`DataSource::Dir`）集成测试：同一合成场景分别以
//! 显式路径（`DataSource::Paths`）与目录发现（`DataSource::Dir`）运行，
//! 逐日账户与逐笔成交必须完全一致——Dir 仅是 Paths 的路径解析前置，
//! 不改变加载与撮合口径。wap 为按需发现：deal_price 非时段价时目录中的
//! wap 文件不影响运行。

mod common;

use common::*;
use rust_bt::{
    load_signal, run, BtParams, DataPaths, DataSource, DealPrice, ExchangeParams, StrategySpec,
    WapKind,
};
use tempfile::TempDir;

const D: [&str; 5] = [
    "2026-01-05",
    "2026-01-06",
    "2026-01-07",
    "2026-01-08",
    "2026-01-09",
];

/// 3 只股票、5 个交易日、零成本零滑点；目录中含 wap.csv（window 3），
/// 供"非时段价时 wap 被忽略"与"时段价时按需发现"两个场景复用。
fn setup() -> TempDir {
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    let mut wap_rows = Vec::new();
    for d in D {
        for inst in ["SH600001", "SH600002", "SZ000001"] {
            bars.push(Bar::new(d, inst));
            wap_rows.push(WapRow::new(d, inst));
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
    write_wap(dir.path(), 3, &wap_rows);
    dir
}

/// 零成本零滑点参数（top_n=2，drop_n=1），数据来源由调用方选择。
fn params(data: DataSource, deal_price: DealPrice) -> BtParams {
    BtParams {
        data,
        start_date: "2026-01-05".into(),
        end_date: "2026-01-10".into(),
        initial_cash: 100_000.0,
        strategy: StrategySpec::topk_dropout(2, 1),
        exchange: ExchangeParams {
            deal_price,
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

fn explicit_paths(dir: &TempDir, wap: bool) -> DataSource {
    DataSource::Paths(DataPaths {
        stock_bar: dir.path().join("stock_bar.csv").to_str().unwrap().into(),
        benchmark: dir.path().join("benchmark.csv").to_str().unwrap().into(),
        wap: wap.then(|| dir.path().join("wap.csv").to_str().unwrap().into()),
    })
}

#[test]
fn dir_source_matches_explicit_paths() {
    let dir = setup();
    let signal = load_signal(dir.path().join("pred.csv").to_str().unwrap()).unwrap();
    let dir_str = dir.path().to_str().unwrap().to_owned();

    // deal_price=open：目录里的 wap.csv 应被忽略（否则 Dir 运行会因
    // "wap 提供但 deal_price 非时段价"告警/差异路径）
    let by_paths = run(
        params(explicit_paths(&dir, false), DealPrice::Open),
        &signal,
    )
    .unwrap();
    let by_dir = run(params(DataSource::Dir(dir_str), DealPrice::Open), &signal).unwrap();
    assert_daily_same(&by_paths.result, &by_dir.result);
    assert_trades_same(&by_paths.result, &by_dir.result);
}

#[test]
fn dir_source_wap_matches_explicit_paths() {
    let dir = setup();
    let signal = load_signal(dir.path().join("pred.csv").to_str().unwrap()).unwrap();
    let dir_str = dir.path().to_str().unwrap().to_owned();

    // deal_price=vwap3：Dir 按需发现 wap.csv，与显式 wap 路径完全同口径
    let wap3 = DealPrice::Wap {
        kind: WapKind::Vwap,
        window: 3,
    };
    let by_paths = run(params(explicit_paths(&dir, true), wap3), &signal).unwrap();
    let by_dir = run(params(DataSource::Dir(dir_str), wap3), &signal).unwrap();
    assert_daily_same(&by_paths.result, &by_dir.result);
    assert_trades_same(&by_paths.result, &by_dir.result);
}
