//! 合成用例：factor 复权调整（10 送 10）、除权日卖出按调整后 volume、
//! 当日新买入不做当日调整（除权日买入的 M 入账 factor 即当日 factor）。
//!
//! 场景（零成本零滑点，cash=150000，top_n=3，drop_n=1）：
//! - d0=01-05 .. d3=01-08，K=SH600031 M=SH600033 L=SH600032
//! - pred d0: K=3 M=2 L=1 -> d1 等额买入 50000 元：
//!   K 5000 股 @10（factor=1）；M 当日除权（factor 1->2，open=5）10000 股 @5，
//!   其入账 last_factor=2，不再参与当日调整（否则 volume 会被错误翻倍）；
//!   L 5000 股 @10
//! - pred d1: K=1 M=3 L=2 -> d2 撮合前复权：K factor 1->2，volume 5000->10000、
//!   cost_price 10->5；卖出最差 K 得 10000 股 @5 = 50000 元
//! 手算：account 恒 150000；d2 起 value=100000（M 50000 + L 50000）、cash=50000。

mod common;

use common::*;
use rust_bt::Side;
use tempfile::TempDir;

const D0: &str = "2026-01-05";
const D1: &str = "2026-01-06";
const D2: &str = "2026-01-07";
const D3: &str = "2026-01-08";

fn ex_day_bar(date: &'static str, inst: &'static str) -> Bar {
    // 除权日：价格减半，pre_close 为除权参考价 5，板 5.5/4.5
    let mut b = Bar::new(date, inst);
    b.open = 5.0;
    b.close = Some(5.0);
    b.pre_close = Some(5.0);
    b.high_limit = Some(5.5);
    b.low_limit = Some(4.5);
    b.factor = 2.0;
    b
}

fn setup() -> (TempDir, Params) {
    let dir = TempDir::new().unwrap();
    let bars = vec![
        // d0：全部常规（factor=1，价格 10）
        Bar::new(D0, "SH600031"),
        Bar::new(D0, "SH600032"),
        Bar::new(D0, "SH600033"),
        // d1：K、L 常规；M 除权（factor 1->2）
        Bar::new(D1, "SH600031"),
        Bar::new(D1, "SH600032"),
        ex_day_bar(D1, "SH600033"),
        // d2：K 除权（factor 1->2）；L 常规；M 除权后常态（factor=2）
        ex_day_bar(D2, "SH600031"),
        Bar::new(D2, "SH600032"),
        ex_day_bar(D2, "SH600033"),
        // d3：K 除权后常态；L 常规；M 除权后常态
        ex_day_bar(D3, "SH600031"),
        Bar::new(D3, "SH600032"),
        ex_day_bar(D3, "SH600033"),
    ];
    write_stock_bar(dir.path(), &bars);
    write_pred(
        dir.path(),
        &[
            (D0, "SH600031", 3.0),
            (D0, "SH600033", 2.0),
            (D0, "SH600032", 1.0),
            (D1, "SH600031", 1.0),
            (D1, "SH600033", 3.0),
            (D1, "SH600032", 2.0),
        ],
    );
    (
        dir,
        Params {
            cash: 150_000.0,
            top_n: 3,
            drop_n: 1,
            end: "2026-01-09".into(),
            ..Default::default()
        },
    )
}

#[test]
fn factor_adjustment() {
    let (dir, params) = setup();
    let r = run_bt(&dir, &params);

    let t = r.trades();
    assert_eq!(t.len(), 4);
    // d1：建仓三笔（M 除权日买入 10000 股 @5）
    check_trade(&t[0], 1, "SH600031", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[1], 1, "SH600033", Side::Buy, 10000.0, 5.0, 10000.0, 5.0, 0.0);
    check_trade(&t[2], 1, "SH600032", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    // d2：K 除权调整（5000 -> 10000）后按调整后 volume 卖出
    check_trade(&t[3], 2, "SH600031", Side::Sell, 10000.0, 5.0, 10000.0, 5.0, 0.0);

    // 逐日账户：市值连续，除权不产生盈亏跳变
    check_daily(
        &r,
        &[
            (150_000.0, 0.0, 150_000.0),
            (150_000.0, 150_000.0, 0.0),
            (150_000.0, 100_000.0, 50_000.0),
            (150_000.0, 100_000.0, 50_000.0),
        ],
    );

    // M 当日新买入不做当日调整：volume 恒 10000（若被错误调整则为 20000）
    let m = hist_row(&r, 1, "SH600033").unwrap();
    assert_f64(m.volume, 10000.0, "M volume d1");
    assert_f64(m.cost_price, 5.0, "M cost d1");
    // K 除权后成本减半：d1 快照（调整前）cost=10；d2 卖出后无快照
    let k = hist_row(&r, 1, "SH600031").unwrap();
    assert_f64(k.cost_price, 10.0, "K cost d1（调整前）");
    assert_positions_cap(&r, 3);
}
