//! 合成用例：期初建仓 + 常规调仓 + count_day + 报表 + 进度条开关不变量。
//!
//! 场景（零成本零滑点，价格恒 10 元）：
//! - d0=01-05 .. d4=01-09，A=SH600001 B=SH600002 C=SZ000001，cash=100000，top_n=2，drop_n=1
//! - pred d0: A=3 B=2 C=1 -> d1 空仓建仓：A、B 各 50000 元 = 5000 股
//! - pred d2（01-07，日历第 3 日）: A=1 B=2 C=3 -> d3 卖出最差 A，买入 C
//! 手算：每日 account 恒 100000；d1 起 value=100000、cash=0。

mod common;

use common::*;
use rust_bt::Side;
use tempfile::TempDir;

const D: [&str; 5] = [
    "2026-01-05",
    "2026-01-06",
    "2026-01-07",
    "2026-01-08",
    "2026-01-09",
];

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
    if with_bench {
        write_bench(dir.path(), &bench_rows(&D, 0.001));
    }
    let params = Params {
        with_benchmark: with_bench,
        ..Default::default()
    };
    (dir, params)
}

#[test]
fn build_and_rebalance() {
    let (dir, params) = setup(false);
    let r = run_bt(&dir, &params);

    // 逐笔成交明细与手算完全一致
    let t = r.trades();
    assert_eq!(t.len(), 4);
    check_trade(&t[0], 1, "SH600001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[1], 1, "SH600002", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[2], 3, "SH600001", Side::Sell, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[3], 3, "SZ000001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);

    // 逐日账户序列与手算完全一致
    check_daily(
        &r,
        &[
            (100_000.0, 0.0, 100_000.0),       // d0 无信号（T-1 无数据），空仓
            (100_000.0, 100_000.0, 0.0),       // d1 建仓
            (100_000.0, 100_000.0, 0.0),       // d2 无信号持有
            (100_000.0, 100_000.0, 0.0),       // d3 调仓（卖 A 买 C）
            (100_000.0, 100_000.0, 0.0),       // d4 持有
        ],
    );

    // count_day：买入成交日记 1，每持有一日 +1；清仓后重买重置
    assert_eq!(hist_row(&r, 1, "SH600001").unwrap().count_day, 1);
    assert_eq!(hist_row(&r, 2, "SH600001").unwrap().count_day, 2);
    assert!(hist_row(&r, 3, "SH600001").is_none()); // A 已清仓
    assert_eq!(hist_row(&r, 4, "SH600002").unwrap().count_day, 4);
    assert_eq!(hist_row(&r, 3, "SZ000001").unwrap().count_day, 1);
    assert_eq!(hist_row(&r, 4, "SZ000001").unwrap().count_day, 2);

    // 不变量：持仓只数 <= top_n
    assert_positions_cap(&r, 2);

    // weight：满仓时各 0.5
    assert_f64(hist_row(&r, 4, "SH600002").unwrap().volume
        * hist_row(&r, 4, "SH600002").unwrap().price
        / r.daily()[4].account, 0.5, "weight B d4");
}

#[test]
fn report_metrics() {
    let (dir, params) = setup(true);
    let r = run_bt(&dir, &params);
    let report = r.gen_report("zz1000", "arithmetic").unwrap();

    // 价格恒定 -> 收益全 0；首日口径 r_0 = 0
    let out = dir.path().join("report_data.csv");
    report.export_data(out.to_str().unwrap()).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "datetime,account,return,total_turnover,turnover,total_cost,cost,value,cash,benchmark"
    );
    assert_eq!(lines.len(), 6); // 表头 + 5 个交易日
    // d0：首日，return=0，无成交 turnover=0，benchmark 当日值 0.001
    let d0: Vec<&str> = lines[1].split(',').collect();
    assert_eq!(d0[0], "2026-01-05");
    assert_f64(d0[1].parse().unwrap(), 100_000.0, "d0 account");
    assert_f64(d0[2].parse().unwrap(), 0.0, "d0 return");
    assert_f64(d0[4].parse().unwrap(), 0.0, "d0 turnover");
    assert_f64(d0[9].parse().unwrap(), 0.001, "d0 benchmark");
    // d1：建仓成交 100000，分母为 d0 总资产 100000 -> turnover = 1.0
    let d1: Vec<&str> = lines[2].split(',').collect();
    assert_f64(d1[3].parse().unwrap(), 100_000.0, "d1 total_turnover");
    assert_f64(d1[4].parse().unwrap(), 1.0, "d1 turnover");
    // d3：调仓双边成交 100000，total_turnover 累计 200000
    let d3: Vec<&str> = lines[4].split(',').collect();
    assert_f64(d3[3].parse().unwrap(), 200_000.0, "d3 total_turnover");
    assert_f64(d3[4].parse().unwrap(), 1.0, "d3 turnover");

    // 收益全 0 -> 年化 0、最大回撤 0；超额 = -0.001/日
    assert_f64(report.derived.annualized_return, 0.0, "annualized_return");
    assert_f64(report.derived.max_drawdown, 0.0, "max_drawdown");
    assert!(report.derived.excess_annualized_return < 0.0);
}

#[test]
fn report_errors() {
    let (dir, params) = setup(true);
    let r = run_bt(&dir, &params);
    // 未知基准名 -> Err
    assert!(r.gen_report("nope", "arithmetic").is_err());
    // 非法 excess_method -> Err
    assert!(r.gen_report("zz1000", "geo").is_err());

    // 基准覆盖不足 -> Err（缺少 01-09）
    let dir2 = TempDir::new().unwrap();
    let (d2_dir, _) = setup(false);
    std::fs::copy(
        d2_dir.path().join("stock_bar.csv"),
        dir2.path().join("stock_bar.csv"),
    )
    .unwrap();
    std::fs::copy(d2_dir.path().join("pred.csv"), dir2.path().join("pred.csv")).unwrap();
    write_bench(dir2.path(), &bench_rows(&D[..4], 0.001));
    let params2 = Params {
        with_benchmark: true,
        ..Default::default()
    };
    let r2 = run_bt(&dir2, &params2);
    assert!(r2.gen_report("zz1000", "arithmetic").is_err());
}

#[test]
fn progress_switch_invariant() {
    // 进度条开关不改变逐日账户序列与成交记录（验收不变量）
    let (dir, mut params) = setup(false);
    params.progress = false;
    let off = run_bt(&dir, &params);
    params.progress = true;
    let on = run_bt(&dir, &params);

    assert_eq!(off.daily().len(), on.daily().len());
    for (a, b) in off.daily().iter().zip(on.daily()) {
        assert_eq!(a.account.to_bits(), b.account.to_bits());
        assert_eq!(a.value.to_bits(), b.value.to_bits());
        assert_eq!(a.cash.to_bits(), b.cash.to_bits());
    }
    assert_eq!(off.trades().len(), on.trades().len());
    for (a, b) in off.trades().iter().zip(on.trades()) {
        assert_eq!(a.stock, b.stock);
        assert_eq!(a.deal_volume.to_bits(), b.deal_volume.to_bits());
        assert_eq!(a.deal_price.to_bits(), b.deal_price.to_bits());
        assert_eq!(a.deal_cost.to_bits(), b.deal_cost.to_bits());
    }
    // elapsed 两态均记录
    let _ = off.elapsed();
    let _ = on.elapsed();
}
