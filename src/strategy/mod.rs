//! Strategy trait 与上下文（架构 §4.6）。
//!
//! 信息边界编译期化：策略只拿到 `StrategyContext`，可见字段即全部可见信息。

pub mod common;
pub mod topk_dropout;

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
    /// 需要其他核减语义的策略覆写本方法。
    fn revise_buy_orders(&self, buys: Vec<Order>, after_sell: &PostSellContext) -> Result<Vec<Order>> {
        Ok(match after_sell.target_positions {
            Some(target) => {
                let keep = target.saturating_sub(after_sell.positions.len());
                buys.into_iter().take(keep).collect()
            }
            None => buys,
        })
    }
}
