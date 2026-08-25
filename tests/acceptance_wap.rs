//! WAP 时段价验收测试：方向价、方向量、缺失行、策略可见价均手算对拍。
//!
//! 约定：零成本零滑点，整百股买入，价格取整数，便于精确断言。

mod common;

use common::{assert_f64, check_trade, Bar, Params, WapRow};
use rust_bt::*;

/// 固定策略：无持仓时买入 10000 股目标股；有持仓时全部卖出。
/// 委托价使用策略可见价（普通模式 = deal_price 列，wap 模式 = pre_close）。
struct BuyThenSell {
    target: Code,
    buy_volume: f64,
}

impl Strategy for BuyThenSell {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision> {
        let t = ctx
            .tradable
            .get(self.target)
            .ok_or_else(|| BtError::InvalidDecision("目标股无行情".into()))?;
        if let Some(pos) = ctx.positions.get(&self.target) {
            if pos.volume > 0.0 && t.sellable() {
                return Ok(Decision {
                    sell_orders: vec![Order::new(self.target, -pos.volume, t.deal_price)],
                    buy_orders: vec![],
                    target_positions: None,
                });
            }
        }
        if t.buyable() {
            return Ok(Decision {
                sell_orders: vec![],
                buy_orders: vec![Order::new(self.target, self.buy_volume, t.deal_price)],
                target_positions: None,
            });
        }
        Ok(Decision::default())
    }
}

/// 构造两日期行情与信号：2026-01-05（周一）、2026-01-06。
fn setup() -> (tempfile::TempDir, Params) {
    let dir = tempfile::tempdir().unwrap();
    let dates = ["2026-01-05", "2026-01-06"];

    // stock_bar：SH600000 pre_close=10，open/close=10；SH600001 仅作占位
    let bars = vec![
        Bar::new(dates[0], "SH600000"),
        Bar::new(dates[0], "SH600001"),
        Bar::new(dates[1], "SH600000"),
        Bar::new(dates[1], "SH600001"),
    ];
    common::write_stock_bar(dir.path(), &bars);

    // pred：2026-01-05 给 SH600000 信号，用于 2026-01-06 决策
    common::write_pred(
        dir.path(),
        &[(dates[0], "SH600000", 1.0), (dates[0], "SH600001", 0.5)],
    );

    let params = Params {
        deal_price: "vwap11".into(),
        cash: 100_000.0,
        start: dates[0].into(),
        end: "2026-01-07".into(),
        volume_threshold: Some(1.0),
        ..Default::default()
    };
    (dir, params)
}

#[test]
fn wap_buy_uses_direction_price_and_volume() {
    let (dir, params) = setup();

    // wap 2026-01-06：买入侧价 9.0，买入量 8000；卖出侧价 11.0，卖出量 10000
    common::write_wap(
        dir.path(),
        11,
        &[
            WapRow {
                date: "2026-01-06",
                inst: "SH600000",
                vwap_buy: 9.0,
                vwap_sell: 11.0,
                twap_buy: 9.5,
                twap_sell: 10.5,
                buy_volume: 8_000.0,
                sell_volume: 10_000.0,
            },
            WapRow::new("2026-01-06", "SH600001"),
        ],
    );

    let target = parse_instrument("SH600000").unwrap();
    let result =
        common::run_bt_with(&dir, &params, Box::new(BuyThenSell { target, buy_volume: 10_000.0 }))
            .unwrap();

    // 2026-01-05 无 T-1 信号，不交易；2026-01-06 买入
    assert_eq!(result.trades().len(), 1);
    let t = &result.trades()[0];
    // 成交价应为 vwap_buy=9.0，成交量受 buy_volume=8000 限制
    check_trade(t, 1, "SH600000", Side::Buy, 10_000.0, 10.0, 8_000.0, 9.0, 0.0);

    // 现金 = 100000 - 8000*9 = 28000；持仓市值 = 8000*close(10) = 80000
    let daily = result.daily();
    assert_eq!(daily.len(), 2);
    assert_f64(daily[1].cash, 28_000.0, "cash");
    assert_f64(daily[1].value, 80_000.0, "value");
}

#[test]
fn wap_sell_uses_direction_price_and_volume() {
    let dir = tempfile::tempdir().unwrap();
    let dates = ["2026-01-05", "2026-01-06", "2026-01-07"];

    let bars: Vec<Bar> = dates
        .iter()
        .flat_map(|d| [Bar::new(d, "SH600000"), Bar::new(d, "SH600001")])
        .collect();
    common::write_stock_bar(dir.path(), &bars);

    // 2026-01-05 信号 -> 2026-01-06 买入；2026-01-06 信号 -> 2026-01-07 卖出
    common::write_pred(
        dir.path(),
        &[
            (dates[0], "SH600000", 1.0),
            (dates[0], "SH600001", 0.5),
            (dates[1], "SH600000", 1.0),
            (dates[1], "SH600001", 0.5),
        ],
    );

    let params = Params {
        deal_price: "vwap11".into(),
        cash: 200_000.0,
        start: dates[0].into(),
        end: "2026-01-08".into(),
        volume_threshold: Some(1.0),
        ..Default::default()
    };

    // 2026-01-06 买入侧价 10，量充足；2026-01-07 卖出侧价 11，卖出量 6000
    common::write_wap(
        dir.path(),
        11,
        &[
            WapRow {
                date: "2026-01-06",
                inst: "SH600000",
                vwap_buy: 10.0,
                vwap_sell: 10.0,
                twap_buy: 10.0,
                twap_sell: 10.0,
                buy_volume: 100_000.0,
                sell_volume: 100_000.0,
            },
            WapRow {
                date: "2026-01-07",
                inst: "SH600000",
                vwap_buy: 9.0,
                vwap_sell: 11.0,
                twap_buy: 9.5,
                twap_sell: 10.5,
                buy_volume: 8_000.0,
                sell_volume: 6_000.0,
            },
            WapRow::new("2026-01-06", "SH600001"),
            WapRow::new("2026-01-07", "SH600001"),
        ],
    );

    let target = parse_instrument("SH600000").unwrap();
    let result =
        common::run_bt_with(&dir, &params, Box::new(BuyThenSell { target, buy_volume: 10_000.0 }))
            .unwrap();

    assert_eq!(result.trades().len(), 2);
    // 2026-01-06 买入 10000@10（现金充足，量充足）
    check_trade(&result.trades()[0], 1, "SH600000", Side::Buy, 10_000.0, 10.0, 10_000.0, 10.0, 0.0);
    // 2026-01-07 卖出受 sell_volume=6000 限制，成交价 vwap_sell=11
    check_trade(
        &result.trades()[1],
        2,
        "SH600000",
        Side::Sell,
        10_000.0,
        10.0,
        6_000.0,
        11.0,
        0.0,
    );

    let daily = result.daily();
    assert_eq!(daily.len(), 3);
    // 2026-01-06：现金 = 200000 - 100000 = 100000，市值 100000
    assert_f64(daily[1].cash, 100_000.0, "day1 cash");
    assert_f64(daily[1].value, 100_000.0, "day1 value");
    // 2026-01-07：现金 = 100000 + 6000*11 = 166000，持仓 4000*10 = 40000
    assert_f64(daily[2].cash, 166_000.0, "day2 cash");
    assert_f64(daily[2].value, 40_000.0, "day2 value");
}

#[test]
fn wap_missing_row_untradable() {
    let (dir, params) = setup();

    // wap 只提供 SH600001，SH600000 缺行 -> 方向量上限为 0，策略判定不可买，无订单
    common::write_wap(
        dir.path(),
        11,
        &[WapRow::new("2026-01-06", "SH600001")],
    );

    struct AssertUntradable {
        target: Code,
    }
    impl Strategy for AssertUntradable {
        fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision> {
            let t = ctx.tradable.get(self.target).unwrap();
            assert!(t.deal_price.is_finite() && t.deal_price > 0.0, "pre_close 仍可见");
            assert_eq!(t.volume_cap, 0.0, "缺 wap 行时买入量上限为 0");
            assert!(!t.buyable(), "缺 wap 行时不可买入");
            Ok(Decision::default())
        }
    }

    let target = parse_instrument("SH600000").unwrap();
    let result =
        common::run_bt_with(&dir, &params, Box::new(AssertUntradable { target })).unwrap();

    assert_eq!(result.trades().len(), 0);
}

#[test]
fn wap_strategy_sees_pre_close_not_wap_price() {
    let (dir, params) = setup();

    // wap 买入侧价 9.0，但策略可见价应为 stock_bar 的 pre_close=10
    common::write_wap(
        dir.path(),
        11,
        &[
            WapRow {
                date: "2026-01-06",
                inst: "SH600000",
                vwap_buy: 9.0,
                vwap_sell: 11.0,
                twap_buy: 8.0,
                twap_sell: 12.0,
                buy_volume: 100_000.0,
                sell_volume: 100_000.0,
            },
            WapRow::new("2026-01-06", "SH600001"),
        ],
    );

    struct AssertPreClose {
        target: Code,
        expected: f64,
    }
    impl Strategy for AssertPreClose {
        fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision> {
            let t = ctx.tradable.get(self.target).unwrap();
            assert_f64(t.deal_price, self.expected, "strategy visible deal_price");
            Ok(Decision::default())
        }
    }

    let target = parse_instrument("SH600000").unwrap();
    common::run_bt_with(&dir, &params, Box::new(AssertPreClose { target, expected: 10.0 })).unwrap();
}

#[test]
fn twap_uses_twap_direction_price() {
    let (dir, params) = setup();
    let mut params = params;
    params.deal_price = "twap11".into();

    common::write_wap(
        dir.path(),
        11,
        &[
            WapRow {
                date: "2026-01-06",
                inst: "SH600000",
                vwap_buy: 9.0,
                vwap_sell: 11.0,
                twap_buy: 8.5,
                twap_sell: 11.5,
                buy_volume: 100_000.0,
                sell_volume: 100_000.0,
            },
            WapRow::new("2026-01-06", "SH600001"),
        ],
    );

    let target = parse_instrument("SH600000").unwrap();
    let result =
        common::run_bt_with(&dir, &params, Box::new(BuyThenSell { target, buy_volume: 5_000.0 }))
            .unwrap();

    assert_eq!(result.trades().len(), 1);
    check_trade(&result.trades()[0], 1, "SH600000", Side::Buy, 5_000.0, 10.0, 5_000.0, 8.5, 0.0);
}
