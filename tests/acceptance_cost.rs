//! 合成用例：min_cost 触发、整手取整不足一手、资金约束反解。
//!
//! 场景：N=SH600041 价格恒 10，d0=01-05、d1=01-06 两日；pred d0: N=1；
//! top_n=1，drop_n=0；open_cost=close_cost=0.001，min_cost=5，零滑点。
//!
//! 手算（买单资金反解：比例费 / 固定费两 regime 取可行解较大者）：
//! - cash=1000：反解 s1=1000/10.01≈99.9（费≈0.1<5 不可行），s2=(1000-5)/10=99.5
//!   （可行）-> 99.5 股 -> 整手不足 100 股 -> 成交 0
//! - cash=2000：s2=(2000-5)/10=199.5 -> 整手 100 股 -> 费=max(1000×0.001,5)=5，
//!   cash=2000-1000-5=995，account=1995
//! - cash=10010：s1=10010/10.01=1000（费=10>=5 可行）-> 1000 股 -> 费=10，
//!   cash=0，account=10000

mod common;

use common::*;
use rust_bt::Side;
use tempfile::TempDir;

fn scenario(cash: f64) -> (TempDir, Params) {
    let dir = TempDir::new().unwrap();
    write_stock_bar(
        dir.path(),
        &[
            Bar::new("2026-01-05", "SH600041"),
            Bar::new("2026-01-06", "SH600041"),
        ],
    );
    write_pred(dir.path(), &[("2026-01-05", "SH600041", 1.0)]);
    (
        dir,
        Params {
            cash,
            top_n: 1,
            drop_n: 0,
            open_cost: 0.001,
            close_cost: 0.001,
            min_cost: 5.0,
            end: "2026-01-07".into(),
            ..Default::default()
        },
    )
}

#[test]
fn insufficient_one_lot() {
    let (dir, params) = scenario(1000.0);
    let r = run_bt(&dir, &params);
    // 委托 100 股，资金反解 99.5 股，整手后成交 0
    let t = r.trades();
    assert_eq!(t.len(), 1);
    check_trade(&t[0], 1, "SH600041", Side::Buy, 100.0, 10.0, 0.0, 0.0, 0.0);
    check_daily(&r, &[(1000.0, 0.0, 1000.0), (1000.0, 0.0, 1000.0)]);
}

#[test]
fn min_cost_fixed_regime() {
    let (dir, params) = scenario(2000.0);
    let r = run_bt(&dir, &params);
    // 委托 200 股，反解 199.5 股，整手 100 股；费 = max(1, 5) = 5（min_cost 触发）
    let t = r.trades();
    assert_eq!(t.len(), 1);
    check_trade(&t[0], 1, "SH600041", Side::Buy, 200.0, 10.0, 100.0, 10.0, 5.0);
    check_daily(&r, &[(2000.0, 0.0, 2000.0), (1995.0, 1000.0, 995.0)]);
}

#[test]
fn proportional_regime() {
    let (dir, params) = scenario(10010.0);
    let r = run_bt(&dir, &params);
    // 委托 1001 股，反解 1000 股（比例费 regime）；费 = max(10, 5) = 10
    let t = r.trades();
    assert_eq!(t.len(), 1);
    check_trade(&t[0], 1, "SH600041", Side::Buy, 1001.0, 10.0, 1000.0, 10.0, 10.0);
    check_daily(&r, &[(10010.0, 0.0, 10010.0), (10000.0, 10000.0, 0.0)]);
}
