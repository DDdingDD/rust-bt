//! 合成用例：TopkStrategy（每日持有 score 前 top_n 只，跌出才卖出）。
//!
//! 与 TopkDropout 的关键差异：仍在 top_n 内的持仓原样保留，不轮换、
//! 不因"已持有"而被排除出买入候选；未持有且当日不可交易的高分股不占名额。
//! 场景均为零成本零滑点、价格恒 10 元，逐笔成交与逐日账户可手算对拍。

mod common;

use common::*;
use rust_bt::{Side, TopkDropoutStrategy, TopkStrategy};
use tempfile::TempDir;

const D: [&str; 5] = [
    "2026-01-05",
    "2026-01-06",
    "2026-01-07",
    "2026-01-08",
    "2026-01-09",
];

/// 三只常规股票 A/B/C、5 个交易日行情；信号由调用方写。
fn setup(pred: &[(&str, &str, f64)]) -> (TempDir, Params) {
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    for d in D {
        for inst in ["SH600001", "SH600002", "SZ000001"] {
            bars.push(Bar::new(d, inst));
        }
    }
    write_stock_bar(dir.path(), &bars);
    write_pred(dir.path(), pred);
    (dir, Params::default())
}

fn run_topk(dir: &TempDir, params: &Params) -> rust_bt::BTResult {
    run_bt_with(dir, params, Box::new(TopkStrategy::new(params.top_n))).unwrap()
}

#[test]
fn rebalance_keeps_overlap() {
    // - pred d0: A=3 B=2 C=1 -> d1 建仓 A、B 各 5000 股
    // - pred d2: A=1 B=2 C=3 -> d3 目标集 {C, B}：仅卖 A 买 C，B 在 top_n 内保留
    let (dir, params) = setup(&[
        (D[0], "SH600001", 3.0),
        (D[0], "SH600002", 2.0),
        (D[0], "SZ000001", 1.0),
        (D[2], "SH600001", 1.0),
        (D[2], "SH600002", 2.0),
        (D[2], "SZ000001", 3.0),
    ]);
    let r = run_topk(&dir, &params);

    let t = r.trades();
    assert_eq!(t.len(), 4);
    check_trade(&t[0], 1, "SH600001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[1], 1, "SH600002", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[2], 3, "SH600001", Side::Sell, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[3], 3, "SZ000001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);

    check_daily(
        &r,
        &[
            (100_000.0, 0.0, 100_000.0),       // d0 无信号，空仓
            (100_000.0, 100_000.0, 0.0),       // d1 建仓
            (100_000.0, 100_000.0, 0.0),       // d2 无信号持有
            (100_000.0, 100_000.0, 0.0),       // d3 调仓（卖 A 买 C，B 保留）
            (100_000.0, 100_000.0, 0.0),       // d4 持有
        ],
    );

    // B 未被轮换：count_day 自 d1 连续累计
    assert_eq!(hist_row(&r, 3, "SH600002").unwrap().count_day, 3);
    assert_eq!(hist_row(&r, 4, "SH600002").unwrap().count_day, 4);
    assert!(hist_row(&r, 3, "SH600001").is_none()); // A 已清仓
    assert_eq!(hist_row(&r, 3, "SZ000001").unwrap().count_day, 1);
    assert_positions_cap(&r, 2);
}

#[test]
fn differs_from_full_dropout() {
    // 同一场景下 TopkDropout(2, 2) 会全换仓：A、B 都卖，候选排除已持有只剩 C，
    // 买 C 10000 股（金额 = 100000 / 1）；Topk 则保留 B。对比锁定语义差异。
    let pred = [
        (D[0], "SH600001", 3.0),
        (D[0], "SH600002", 2.0),
        (D[0], "SZ000001", 1.0),
        (D[2], "SH600001", 1.0),
        (D[2], "SH600002", 2.0),
        (D[2], "SZ000001", 3.0),
    ];
    let (dir1, params1) = setup(&pred);
    let topk = run_topk(&dir1, &params1);

    let (dir2, params2) = setup(&pred);
    let dropout = run_bt_with(
        &dir2,
        &params2,
        Box::new(TopkDropoutStrategy::new(2, 2)),
    )
    .unwrap();

    assert_eq!(topk.trades().len(), 4);
    let dt = dropout.trades();
    assert_eq!(dt.len(), 5);
    check_trade(&dt[2], 3, "SH600001", Side::Sell, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&dt[3], 3, "SH600002", Side::Sell, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&dt[4], 3, "SZ000001", Side::Buy, 10000.0, 10.0, 10000.0, 10.0, 0.0);
    // dropout d3 清仓 B；topk d3 仍持有 B
    assert!(hist_row(&dropout, 3, "SH600002").is_none());
    assert!(hist_row(&topk, 3, "SH600002").is_some());
}

#[test]
fn no_score_holding_sold() {
    // 持仓股当日无 score -> 不在目标集 -> 卖出（与 TopkDropout 同口径）
    // - pred d0: A=3 B=2 -> d1 建仓 A、B
    // - pred d2: B=2 C=3（A 掉出信号）-> d3 卖 A 买 C，B 保留
    let (dir, params) = setup(&[
        (D[0], "SH600001", 3.0),
        (D[0], "SH600002", 2.0),
        (D[2], "SH600002", 2.0),
        (D[2], "SZ000001", 3.0),
    ]);
    let r = run_topk(&dir, &params);

    let t = r.trades();
    assert_eq!(t.len(), 4);
    check_trade(&t[2], 3, "SH600001", Side::Sell, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[3], 3, "SZ000001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    assert!(hist_row(&r, 3, "SH600001").is_none());
    assert_eq!(hist_row(&r, 3, "SH600002").unwrap().count_day, 3);
    assert_positions_cap(&r, 2);
}

#[test]
fn untradable_excluded_from_target() {
    // 未持有且当日不可交易的高分股不占 top_n 名额：
    // - pred d0: A=3 B=2 C=1 -> d1 建仓 A、B
    // - pred d2: C=3 A=2 B=1，但执行日 d3 C 停牌 -> C 不进排名宇宙，
    //   目标集 {A, B}：d3 无任何成交（B 不被 C 挤出）
    // - pred d3: C=3 B=2 A=1 -> d4 C 恢复交易，目标集 {C, B}：卖 A 买 C
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    for d in D {
        for inst in ["SH600001", "SH600002", "SZ000001"] {
            let mut b = Bar::new(d, inst);
            if d == D[3] && inst == "SZ000001" {
                b.paused = 1; // d3（pred d2 的执行日）C 停牌
            }
            bars.push(b);
        }
    }
    write_stock_bar(dir.path(), &bars);
    write_pred(
        dir.path(),
        &[
            (D[0], "SH600001", 3.0),
            (D[0], "SH600002", 2.0),
            (D[0], "SZ000001", 1.0),
            (D[2], "SZ000001", 3.0),
            (D[2], "SH600001", 2.0),
            (D[2], "SH600002", 1.0),
            (D[3], "SZ000001", 3.0),
            (D[3], "SH600002", 2.0),
            (D[3], "SH600001", 1.0),
        ],
    );
    let r = run_topk(&dir, &Params::default());

    let t = r.trades();
    assert_eq!(t.len(), 4);
    check_trade(&t[0], 1, "SH600001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[1], 1, "SH600002", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    // d3 无成交（C 停牌不占名额，A、B 均在目标集内）；d4 卖 A 买 C
    check_trade(&t[2], 4, "SH600001", Side::Sell, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[3], 4, "SZ000001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);

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
    assert_eq!(hist_row(&r, 3, "SH600002").unwrap().count_day, 3);
    assert!(hist_row(&r, 4, "SH600001").is_none());
    assert_eq!(hist_row(&r, 4, "SZ000001").unwrap().count_day, 1);
    assert_positions_cap(&r, 2);
}

#[test]
fn suspended_holding_kept_while_in_top_n() {
    // 已持有的股票当日停牌但有 score：仍参与排名，在 top_n 内则保留
    // （不产生卖单；等复牌后再按排名决定去留）
    // - pred d0: A=3 B=2 -> d1 建仓 A、B
    // - pred d2: A=3 B=2 C=1，执行日 d3 A 停牌 -> A 仍在 top_n 内：d3 无成交
    // - pred d3: C=3 B=2 A=1 -> d4 A 复牌，跌出 top_n：卖 A 买 C
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    for d in D {
        for inst in ["SH600001", "SH600002", "SZ000001"] {
            let mut b = Bar::new(d, inst);
            if d == D[3] && inst == "SH600001" {
                b.paused = 1; // d3 A 停牌
            }
            bars.push(b);
        }
    }
    write_stock_bar(dir.path(), &bars);
    write_pred(
        dir.path(),
        &[
            (D[0], "SH600001", 3.0),
            (D[0], "SH600002", 2.0),
            (D[2], "SH600001", 3.0),
            (D[2], "SH600002", 2.0),
            (D[2], "SZ000001", 1.0),
            (D[3], "SZ000001", 3.0),
            (D[3], "SH600002", 2.0),
            (D[3], "SH600001", 1.0),
        ],
    );
    let r = run_topk(&dir, &Params::default());

    let t = r.trades();
    assert_eq!(t.len(), 4);
    check_trade(&t[0], 1, "SH600001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[1], 1, "SH600002", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    // d3 无成交（A 停牌但在 top_n 内保留）；d4 卖 A 买 C
    check_trade(&t[2], 4, "SH600001", Side::Sell, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[3], 4, "SZ000001", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);

    // A 停牌日 d3 仍在持仓快照中（停牌估值沿用），d4 清仓
    assert!(hist_row(&r, 3, "SH600001").is_some());
    assert!(hist_row(&r, 4, "SH600001").is_none());
    assert_eq!(hist_row(&r, 4, "SH600002").unwrap().count_day, 4);
    assert_positions_cap(&r, 2);
}
