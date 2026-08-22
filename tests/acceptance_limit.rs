//! 合成用例：涨跌停拦截 + 两阶段撮合核减买单。
//!
//! 场景（零成本零滑点，cash=100000，top_n=2，drop_n=1）：
//! - d0=01-05 .. d3=01-08，D=SH600011 E=SH600012 F=SZ000011 G=SZ000012
//! - pred d0: D=4 E=3 F=2 -> d1：D 涨停（open=11，pre_close=10，板 11/9）被候选过滤，
//!   买入 E、F 各 5000 股 @10，cash=0
//! - pred d2（01-07）: E=1 F=2 G=3 -> d3：E 跌停（open=9，板 11/9）卖出拦截成交 0；
//!   核减：卖出后实际持仓仍 2 只 = top_n，买单 G 被丢弃（不产生 trades 行）
//! 手算：d3 末 E 估值 9 元 -> value = 45000+50000 = 95000，account=95000。

mod common;

use common::*;
use rust_bt::Side;
use tempfile::TempDir;

const D0: &str = "2026-01-05";
const D1: &str = "2026-01-06";
const D2: &str = "2026-01-07";
const D3: &str = "2026-01-08";

fn setup() -> (TempDir, Params) {
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    // d0：全部常规
    for inst in ["SH600011", "SH600012", "SZ000011", "SZ000012"] {
        bars.push(Bar::new(D0, inst));
    }
    // d1：D 涨停（open=close=11，pre_close=10，板 11/9 -> change=+10% >= 9.85% 触发线）
    let mut d_limit_up = Bar::new(D1, "SH600011");
    d_limit_up.open = 11.0;
    d_limit_up.close = Some(11.0);
    bars.push(d_limit_up);
    for inst in ["SH600012", "SZ000011", "SZ000012"] {
        bars.push(Bar::new(D1, inst));
    }
    // d2：全部常规
    for inst in ["SH600011", "SH600012", "SZ000011", "SZ000012"] {
        bars.push(Bar::new(D2, inst));
    }
    // d3：E 跌停（open=close=9 -> change=-10% <= -9.85% 触发线）
    let mut e_limit_down = Bar::new(D3, "SH600012");
    e_limit_down.open = 9.0;
    e_limit_down.close = Some(9.0);
    bars.push(e_limit_down);
    for inst in ["SH600011", "SZ000011", "SZ000012"] {
        bars.push(Bar::new(D3, inst));
    }
    write_stock_bar(dir.path(), &bars);
    write_pred(
        dir.path(),
        &[
            (D0, "SH600011", 4.0),
            (D0, "SH600012", 3.0),
            (D0, "SZ000011", 2.0),
            (D2, "SH600012", 1.0),
            (D2, "SZ000011", 2.0),
            (D2, "SZ000012", 3.0),
        ],
    );
    (
        dir,
        Params {
            end: "2026-01-09".into(),
            ..Default::default()
        },
    )
}

#[test]
fn limit_and_two_phase_reduction() {
    let (dir, params) = setup();
    let r = run_bt(&dir, &params);

    let t = r.trades();
    assert_eq!(t.len(), 3);
    // d1：D 涨停被过滤不产生订单；E、F 成交
    check_trade(&t[0], 1, "SH600012", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[1], 1, "SZ000011", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    // d3：E 跌停卖出拦截（委托 5000 @ 跌停价 9，成交 0）
    check_trade(&t[2], 3, "SH600012", Side::Sell, 5000.0, 9.0, 0.0, 0.0, 0.0);
    // G 的买单被核减丢弃，不产生 trades 行（t.len() == 3 已含此断言）

    // 逐日账户
    check_daily(
        &r,
        &[
            (100_000.0, 0.0, 100_000.0),     // d0 空仓
            (100_000.0, 100_000.0, 0.0),     // d1 买入 E、F
            (100_000.0, 100_000.0, 0.0),     // d2 持有
            (95_000.0, 95_000.0, 0.0),       // d3 E 估值 9 元
        ],
    );

    // E 未卖出，继续占坑：d3 持仓 E、F 两只
    assert!(hist_row(&r, 3, "SH600012").is_some());
    assert!(hist_row(&r, 3, "SZ000011").is_some());
    assert!(hist_row(&r, 3, "SZ000012").is_none());
    assert_positions_cap(&r, 2);
}
