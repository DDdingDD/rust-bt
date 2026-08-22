//! 合成用例：停牌拦截 + 停牌 / 退市估值沿用最近有效收盘价。
//!
//! 场景（零成本零滑点，cash=100000，top_n=2，drop_n=1）：
//! - d0=01-05 .. d4=01-09，H=SH600021 I=SH600022 J=SH600023
//! - pred d0: H=3 I=2 -> d1 买入 H、I 各 5000 股 @10；d1 H 收盘 12（有效）
//! - pred d1: H=1 I=2 J=3 -> d2：H 停牌（paused=1，close 列给 11 但不采用）
//!   卖出拦截成交 0，J 买单被核减丢弃；H 估值沿用 12
//! - pred d2: H=1 I=2 J=3 -> d3 起 H 退市（无行情行）：卖出仍拦截（当日无行情），
//!   估值沿用 12 直至期末
//! 手算：d1 起 account 恒 110000（H 5000×12 + I 5000×10）。

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

fn setup() -> (TempDir, Params) {
    let dir = TempDir::new().unwrap();
    let mut bars = Vec::new();
    // d0：H、I 常规（J 无行情，候选不涉及）
    bars.push(Bar::new(D[0], "SH600021"));
    bars.push(Bar::new(D[0], "SH600022"));
    // d1：H 收盘 12（有效收盘价）；I 常规
    let mut h1 = Bar::new(D[1], "SH600021");
    h1.close = Some(12.0);
    bars.push(h1);
    bars.push(Bar::new(D[1], "SH600022"));
    // d2：H 停牌（paused=1，close 列 11 不采用）；I、J 常规
    let mut h2 = Bar::new(D[2], "SH600021");
    h2.paused = 1;
    h2.close = Some(11.0);
    bars.push(h2);
    bars.push(Bar::new(D[2], "SH600022"));
    bars.push(Bar::new(D[2], "SH600023"));
    // d3、d4：H 退市无行情行；I、J 常规
    for d in [D[3], D[4]] {
        bars.push(Bar::new(d, "SH600022"));
        bars.push(Bar::new(d, "SH600023"));
    }
    write_stock_bar(dir.path(), &bars);
    write_pred(
        dir.path(),
        &[
            (D[0], "SH600021", 3.0),
            (D[0], "SH600022", 2.0),
            (D[1], "SH600021", 1.0),
            (D[1], "SH600022", 2.0),
            (D[1], "SH600023", 3.0),
            (D[2], "SH600021", 1.0),
            (D[2], "SH600022", 2.0),
            (D[2], "SH600023", 3.0),
        ],
    );
    (dir, Params::default())
}

#[test]
fn paused_and_delisted_valuation() {
    let (dir, params) = setup();
    let r = run_bt(&dir, &params);

    let t = r.trades();
    assert_eq!(t.len(), 4);
    // d1：建仓 H、I
    check_trade(&t[0], 1, "SH600021", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[1], 1, "SH600022", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    // d2：H 停牌卖出拦截（委托价取当日 open=10，成交 0）
    check_trade(&t[2], 2, "SH600021", Side::Sell, 5000.0, 10.0, 0.0, 0.0, 0.0);
    // d3：H 退市无行情，卖出拦截（委托价兜底为最近有效收盘价 12，成交 0）
    check_trade(&t[3], 3, "SH600021", Side::Sell, 5000.0, 12.0, 0.0, 0.0, 0.0);
    // J 买单两日均被核减丢弃，不产生 trades 行

    // 逐日账户：H 估值沿用最近有效收盘价 12（停牌行 close=11 不采用）
    check_daily(
        &r,
        &[
            (100_000.0, 0.0, 100_000.0),
            (110_000.0, 110_000.0, 0.0),
            (110_000.0, 110_000.0, 0.0),
            (110_000.0, 110_000.0, 0.0),
            (110_000.0, 110_000.0, 0.0),
        ],
    );

    // 停牌 / 退市持仓照常输出，price 沿用 12，count_day 照常 +1
    for (day, cd) in [(2, 2u32), (3, 3), (4, 4)] {
        let h = hist_row(&r, day, "SH600021").unwrap();
        assert_f64(h.price, 12.0, "H price 沿用");
        assert_eq!(h.count_day, cd);
    }
    assert_positions_cap(&r, 2);
}
