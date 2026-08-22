//! 合成用例：deal_price 列无效（缺失 / NaN）当日该股不可交易。
//!
//! 场景（deal_price="vwap"，零成本零滑点，cash=100000，top_n=2，drop_n=0）：
//! - d0=01-05、d1=01-06，P=SH600051 Q=SH600052
//! - pred d0: P=2 Q=1 -> d1：P 当日 vwap 缺失 -> 不可交易，被候选过滤；
//!   n_buy=2 收缩到可用候选 1 只，全部资金 100000 买入 Q = 10000 股 @10

mod common;

use common::*;
use rust_bt::Side;
use tempfile::TempDir;

fn setup() -> (TempDir, Params) {
    let dir = TempDir::new().unwrap();
    let mut p1 = Bar::new("2026-01-06", "SH600051");
    p1.vwap = None; // vwap 缺失（无成交日 money/volume 无效）
    write_stock_bar(
        dir.path(),
        &[
            Bar::new("2026-01-05", "SH600051"),
            Bar::new("2026-01-05", "SH600052"),
            p1,
            Bar::new("2026-01-06", "SH600052"),
        ],
    );
    write_pred(
        dir.path(),
        &[
            ("2026-01-05", "SH600051", 2.0),
            ("2026-01-05", "SH600052", 1.0),
        ],
    );
    (
        dir,
        Params {
            deal_price: "vwap".into(),
            drop_n: 0,
            end: "2026-01-07".into(),
            ..Default::default()
        },
    )
}

#[test]
fn invalid_deal_price_untradable() {
    let (dir, params) = setup();
    let r = run_bt(&dir, &params);

    // P 不产生任何订单；Q 以全部资金成交
    let t = r.trades();
    assert_eq!(t.len(), 1);
    check_trade(&t[0], 1, "SH600052", Side::Buy, 10000.0, 10.0, 10000.0, 10.0, 0.0);

    check_daily(&r, &[(100_000.0, 0.0, 100_000.0), (100_000.0, 100_000.0, 0.0)]);
    assert!(hist_row(&r, 1, "SH600051").is_none());
    assert!(hist_row(&r, 1, "SH600052").is_some());
}
