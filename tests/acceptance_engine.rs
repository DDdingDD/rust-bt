//! 引擎级行为用例：卖出超额截断、同股买卖冲突报错、同股多笔买单合并。

mod common;

use common::*;
use rust_bt::*;
use tempfile::TempDir;

/// 脚本策略：第 n 次调用输出预定义（code, 带符号数量）订单，价格取当日 deal_price。
struct ScriptStrategy {
    calls: usize,
    steps: Vec<Vec<(Code, f64)>>,
}

impl ScriptStrategy {
    fn new(steps: Vec<Vec<(Code, f64)>>) -> Self {
        Self { calls: 0, steps }
    }
}

impl Strategy for ScriptStrategy {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision> {
        let step = self.steps.get(self.calls).cloned().unwrap_or_default();
        self.calls += 1;
        let mut d = Decision::default();
        for (code, vol) in step {
            let price = ctx
                .tradable
                .get(code)
                .filter(|t| t.deal_price.is_finite() && t.deal_price > 0.0)
                .map(|t| t.deal_price)
                .unwrap_or(10.0);
            let order = Order::new(code, vol, price);
            if vol > 0.0 {
                d.buy_orders.push(order);
            } else {
                d.sell_orders.push(order);
            }
        }
        Ok(d)
    }
}

const S: &str = "SH600061";

fn setup() -> TempDir {
    let dir = TempDir::new().unwrap();
    write_stock_bar(
        dir.path(),
        &[
            Bar::new("2026-01-05", S),
            Bar::new("2026-01-06", S),
            Bar::new("2026-01-07", S),
        ],
    );
    // 两个信号日 -> d1、d2 两次决策
    write_pred(dir.path(), &[("2026-01-05", S, 1.0), ("2026-01-06", S, 1.0)]);
    dir
}

fn params() -> Params {
    Params {
        end: "2026-01-08".into(),
        ..Default::default()
    }
}

#[test]
fn sell_clipped_to_position() {
    let dir = setup();
    let s = parse_instrument(S).unwrap();
    // d1 买 1000 股；d2 委托卖出 2000 股（超持仓）-> 截断至 1000 股
    let strategy = Box::new(ScriptStrategy::new(vec![
        vec![(s, 1000.0)],
        vec![(s, -2000.0)],
    ]));
    let r = run_bt_with(&dir, &params(), strategy).unwrap();

    let t = r.trades();
    assert_eq!(t.len(), 2);
    check_trade(&t[0], 1, S, Side::Buy, 1000.0, 10.0, 1000.0, 10.0, 0.0);
    // 委托量保留 2000，成交量截断为持仓量 1000
    check_trade(&t[1], 2, S, Side::Sell, 2000.0, 10.0, 1000.0, 10.0, 0.0);
    check_daily(
        &r,
        &[
            (100_000.0, 0.0, 100_000.0),
            (100_000.0, 10_000.0, 90_000.0),
            (100_000.0, 0.0, 100_000.0),
        ],
    );
}

#[test]
fn buy_sell_conflict_rejected() {
    let dir = setup();
    let s = parse_instrument(S).unwrap();
    // 同一交易步内买、卖同一股票 -> Err（策略错误）
    let strategy = Box::new(ScriptStrategy::new(vec![vec![(s, 1000.0), (s, -1000.0)]]));
    let result = run_bt_with(&dir, &params(), strategy);
    assert!(
        matches!(result, Err(BtError::InvalidDecision(_))),
        "同股买卖冲突应报 InvalidDecision"
    );
}

#[test]
fn duplicate_buys_merged() {
    let dir = setup();
    let s = parse_instrument(S).unwrap();
    // 同股两笔买单合并为一笔（数量相加）
    let strategy = Box::new(ScriptStrategy::new(vec![vec![(s, 100.0), (s, 100.0)]]));
    let r = run_bt_with(&dir, &params(), strategy).unwrap();

    let t = r.trades();
    assert_eq!(t.len(), 1);
    check_trade(&t[0], 1, S, Side::Buy, 200.0, 10.0, 200.0, 10.0, 0.0);
}
