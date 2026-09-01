//! 合成用例：停牌日的 factor 复权调整。
//!
//! 覆盖 `position.rs` 中 `adjust_entry_factor` 对 `price` 的调整：
//! 若只调 volume/cost_price 而不调 price，停牌除权日市值会按 factor 跳变。

mod common;

use common::*;
use rust_bt::Side;
use tempfile::TempDir;

const DM1: &str = "2026-01-02"; // 信号日，用于 d0 建仓
const D0: &str = "2026-01-05";
const D1: &str = "2026-01-06";
const D2: &str = "2026-01-07";
const D3: &str = "2026-01-08";

fn paused_ex_day_bar(date: &'static str, inst: &'static str) -> Bar {
    // 停牌除权日：factor 1->2，pre_close 按除权参考价 5；open 同步为 5，close 缺失。
    let mut b = Bar::new(date, inst);
    b.open = 5.0;
    b.close = None;
    b.pre_close = Some(5.0);
    b.high_limit = Some(5.5);
    b.low_limit = Some(4.5);
    b.factor = 2.0;
    b.paused = 1;
    b
}

fn post_split_bar(date: &'static str, inst: &'static str) -> Bar {
    // 复权后常态交易日：价格基准已切到 5，factor 保持 2。
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
        // dm1：常规（factor=1，价格 10），仅用于产生 d0 建仓信号
        Bar::new(DM1, "SH600031"),
        Bar::new(DM1, "SH600032"),
        // d0：常规，按 dm1 信号建仓 K、L
        Bar::new(D0, "SH600031"),
        Bar::new(D0, "SH600032"),
        // d1：K 停牌除权（factor 1->2），L 常规
        paused_ex_day_bar(D1, "SH600031"),
        Bar::new(D1, "SH600032"),
        // d2/d3：K 复权后常态，L 常规
        post_split_bar(D2, "SH600031"),
        Bar::new(D2, "SH600032"),
        post_split_bar(D3, "SH600031"),
        Bar::new(D3, "SH600032"),
    ];
    write_stock_bar(dir.path(), &bars);
    write_pred(
        dir.path(),
        &[
            // d0 建仓信号
            (DM1, "SH600031", 2.0),
            (DM1, "SH600032", 1.0),
            // d1/d2/d3 持仓期内信号，保持 K、L 在 top_n 内
            (D0, "SH600031", 2.0),
            (D0, "SH600032", 1.0),
            (D1, "SH600031", 2.0),
            (D1, "SH600032", 1.0),
            (D2, "SH600031", 2.0),
            (D2, "SH600032", 1.0),
        ],
    );
    (
        dir,
        Params {
            cash: 100_000.0,
            top_n: 2,
            drop_n: 0,
            start: "2026-01-05".into(),
            end: "2026-01-09".into(),
            ..Default::default()
        },
    )
}

#[test]
fn paused_factor_adjustment_keeps_value_continuous() {
    let (dir, params) = setup();
    let r = run_bt(&dir, &params);

    // d0 建仓 K、L 各 5000 股 @10（交易日历从 dm1 开始，d0 对应 day_idx=1）
    let t = r.trades();
    assert_eq!(t.len(), 2);
    check_trade(&t[0], 1, "SH600031", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);
    check_trade(&t[1], 1, "SH600032", Side::Buy, 5000.0, 10.0, 5000.0, 10.0, 0.0);

    // 市值应连续：停牌除权日 account 仍为 100000，不应跳变
    check_daily(
        &r,
        &[
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
            (100_000.0, 100_000.0, 0.0),
        ],
    );

    // K 停牌日复权后快照：volume 翻倍、cost_price 减半、price 同步减半
    let k = hist_row(&r, 2, "SH600031").unwrap();
    assert_f64(k.volume, 10_000.0, "K volume after split");
    assert_f64(k.cost_price, 5.0, "K cost_price after split");
    assert_f64(k.price, 5.0, "K price adjusted on paused ex-day");

    // L 未发生复权，保持原状
    let l = hist_row(&r, 2, "SH600032").unwrap();
    assert_f64(l.volume, 5_000.0, "L volume unchanged");
    assert_f64(l.cost_price, 10.0, "L cost_price unchanged");
    assert_f64(l.price, 10.0, "L price unchanged");
}
