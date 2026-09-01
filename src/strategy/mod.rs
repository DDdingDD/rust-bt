//! Strategy trait 与上下文（架构 §4.6）。
//!
//! 信息边界编译期化：策略只拿到 `StrategyContext`，可见字段即全部可见信息。

pub mod common;
pub mod topk;
pub mod topk_dropout;

pub use topk::TopkStrategy;
pub use topk_dropout::TopkDropoutStrategy;

use crate::error::Result;
use crate::order::{Decision, Order};
use crate::position::Positions;
use crate::signal::SignalDay;
use crate::types::{DayIdx, TradableInfo};

/// 策略在 T_exec 日可见的全部信息（规范"信息边界"的编译期落实）。
pub struct StrategyContext<'a> {
    /// T−1 日信号（score，已剥离 ret）。无信号日 Backtest 直接跳过决策，
    /// gen_decision 不会被以"无信号"状态调用。
    pub signal: &'a SignalDay,
    /// 复权调整后的当前持仓
    pub positions: &'a Positions,
    /// 可用现金
    pub cash: f64,
    /// T_exec 日各股可交易性
    pub tradable: TradableInfo<'a>,
    /// 当日索引
    pub day: DayIdx,
}

/// 核减钩子可见的卖出后状态（均为 T_exec 日合法信息：卖出成交结果当日可得、
/// 回款当日可用，不破坏信息边界）。
pub struct PostSellContext<'a> {
    /// 卖出成交后的实际持仓
    pub positions: &'a Positions,
    /// 含卖出回款（已扣费用）
    pub cash: f64,
    /// 与决策时同一份当日视图
    pub tradable: TradableInfo<'a>,
    /// 卖单成交结果（含部分成交 / 未成交回填）
    pub filled_sells: &'a [Order],
    /// 来自 `Decision::target_positions`：买单只数核减目标（None 表示默认不核减）
    pub target_positions: Option<usize>,
}

/// 策略接口：输入 T−1 日信号与 T_exec 日账户状态，输出 Decision。
pub trait Strategy {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision>;

    /// 阶段一（卖出全部撮合完成）之后、阶段二（买入撮合）之前的买单修正钩子。
    ///
    /// 默认实现：按 `Decision.target_positions` 截断买单（None 则原样返回）——
    /// 即 TopkDropout 的核减语义（`top_n − 卖出成交后实际持仓数`，从尾部丢弃）。
    /// 当实际发生核减（保留数 < 原买单数）时，将剩余买单重新等权分配实际可用现金
    /// `after_sell.cash`，避免被截断的买单仍按原 n_buy 口径只使用部分资金。
    /// 需要其他核减语义的策略覆写本方法。
    fn revise_buy_orders(&self, buys: Vec<Order>, after_sell: &PostSellContext) -> Result<Vec<Order>> {
        Ok(match after_sell.target_positions {
            Some(target) => {
                let keep = target.saturating_sub(after_sell.positions.len());
                if keep == 0 || keep >= buys.len() {
                    // 未发生有效核减：直接返回（空或全保留）
                    return Ok(buys.into_iter().take(keep).collect());
                }
                let mut kept: Vec<Order> = buys.into_iter().take(keep).collect();
                let cash = after_sell.cash;
                if cash > 0.0 {
                    let per = cash / kept.len() as f64;
                    for o in &mut kept {
                        if o.price.is_finite() && o.price > 0.0 {
                            o.volume = per / o.price;
                        } else {
                            o.volume = 0.0;
                        }
                    }
                } else {
                    for o in &mut kept {
                        o.volume = 0.0;
                    }
                }
                kept
            }
            None => buys,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::Order;
    use crate::position::Positions;

    struct DummyStrategy;

    impl Strategy for DummyStrategy {
        fn gen_decision(&mut self,
            _ctx: &StrategyContext,
        ) -> crate::error::Result<Decision> {
            Ok(Decision::default())
        }
    }

    #[test]
    fn revise_renormalizes_on_truncation() {
        // 原 2 只买单各按 50000 元生成；核减为 1 只后应使用全部 100000 元
        let buys = vec![
            Order::new(600_001, 5000.0, 10.0), //  intending 50000
            Order::new(600_002, 2500.0, 20.0), //  intending 50000
        ];
        let positions = Positions::new();
        let tradable = TradableInfo {
            codes: &[],
            suspended: &[],
            limit_buy: &[],
            limit_sell: &[],
            volume_cap: &[],
            sell_volume_cap: &[],
            deal_price: &[],
            is_st: &[],
        };
        let ctx = PostSellContext {
            positions: &positions,
            cash: 100_000.0,
            tradable,
            filled_sells: &[],
            target_positions: Some(1),
        };
        let kept = DummyStrategy.revise_buy_orders(buys, &ctx).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].stock, 600_001);
        assert!((kept[0].volume - 10_000.0).abs() < 1e-9);
    }

    #[test]
    fn revise_no_truncation_keeps_volumes() {
        let buys = vec![
            Order::new(600_001, 5000.0, 10.0),
            Order::new(600_002, 2500.0, 20.0),
        ];
        let positions = Positions::new();
        let tradable = TradableInfo {
            codes: &[],
            suspended: &[],
            limit_buy: &[],
            limit_sell: &[],
            volume_cap: &[],
            sell_volume_cap: &[],
            deal_price: &[],
            is_st: &[],
        };
        let ctx = PostSellContext {
            positions: &positions,
            cash: 100_000.0,
            tradable,
            filled_sells: &[],
            target_positions: Some(2),
        };
        let kept = DummyStrategy.revise_buy_orders(buys, &ctx).unwrap();
        assert_eq!(kept.len(), 2);
        assert!((kept[0].volume - 5000.0).abs() < 1e-9);
        assert!((kept[1].volume - 2500.0).abs() < 1e-9);
    }
}
