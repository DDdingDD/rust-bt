//! TopkStrategy（规范"内置策略"）。
//!
//! 目标持仓：每日持有预测分数最高的 `top_n` 只股票（不可交易的除外）。
//! 与 TopkDropout 的区别：排名跌出 `top_n` 的持仓才卖出，仍在 `top_n` 内的
//! 持仓原样保留——不每日轮换、不因"已持有"而被排除出买入候选。
//! 两阶段核减（卖不掉继续占坑）由 Backtest 编排、trait 默认钩子执行。

use std::collections::HashSet;

use crate::error::Result;
use crate::order::{Decision, Order};
use crate::strategy::common;
use crate::strategy::{Strategy, StrategyContext};
use crate::types::Code;

/// Topk 策略：每日持有 score 前 `top_n` 只。
pub struct TopkStrategy {
    /// 目标持仓只数（>= 1）
    top_n: usize,
    /// 是否过滤 ST 股（只限制买入）
    forbid_st: bool,
}

impl TopkStrategy {
    /// `forbid_st` 默认 false，builder 方法覆写。
    ///
    /// # Panics
    /// `top_n < 1` 属调用方编程错误。
    pub fn new(top_n: usize) -> Self {
        assert!(top_n >= 1, "top_n 须 >= 1，收到: {top_n}");
        Self {
            top_n,
            forbid_st: false,
        }
    }

    pub fn with_forbid_st(mut self, forbid_st: bool) -> Self {
        self.forbid_st = forbid_st;
        self
    }

    pub fn top_n(&self) -> usize {
        self.top_n
    }
}

impl Strategy for TopkStrategy {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision> {
        // ---- 1. 目标集合 T ----
        // 排名宇宙：当日有 score 且（当前已持有 或 当日可买入）的股票——
        // 未持有且不可交易的股票进不来也留不住，不占 top_n 名额；
        // 已持有的股票即使当日停牌/涨停也参与排名（避免"停牌一天就被卖出"）。
        // forbid_st 只限制买入：未持有的 ST 股不进宇宙，已持有的照常排名。
        let mut ranked: Vec<(Code, f64)> = ctx
            .signal
            .codes
            .iter()
            .copied()
            .zip(ctx.signal.scores.iter().copied())
            .filter(|(c, _)| {
                ctx.positions.contains_key(c)
                    || (ctx.tradable.get(*c).is_some_and(|t| t.buyable())
                        && !(self.forbid_st && ctx.tradable.get(*c).is_some_and(|t| t.is_st)))
            })
            .collect();
        common::sort_by_score_desc(&mut ranked);
        ranked.truncate(self.top_n);
        let target: HashSet<Code> = ranked.iter().map(|(c, _)| *c).collect();

        // ---- 2. 卖出：持仓中不在 T 的全部卖出（含当日无 score 的持仓）----
        // 按代码升序保证确定性。卖不掉的（停牌/跌停/量裁剪）成交为 0、
        // 保留持仓并继续占坑，由默认核减钩子收缩买单。
        let mut sell_codes: Vec<Code> = ctx
            .positions
            .keys()
            .filter(|c| !target.contains(c))
            .copied()
            .collect();
        sell_codes.sort_unstable();
        let sell_orders: Vec<Order> = sell_codes
            .iter()
            .map(|c| {
                let volume = ctx.positions[c].volume;
                // 委托价格取当日 deal_price 列；当日无行情 / 无效时用最近有效收盘价兜底
                // （此类卖单会被撮合层裁为 0，价格不参与成交）
                let price = ctx
                    .tradable
                    .get(*c)
                    .filter(|t| t.deal_price.is_finite() && t.deal_price > 0.0)
                    .map(|t| t.deal_price)
                    .unwrap_or(ctx.positions[c].price);
                Order::new(*c, -volume, price)
            })
            .collect();

        // ---- 3. 买入：T 中未持有的股票（按构造必然当日可买入），等权分配 ----
        // n_buy = |T \ 持仓| <= top_n - 卖出后保留持仓数，天然不超配；
        // 金额一次性确定：后续核减只丢单、不重新分配。
        let buy_candidates: Vec<(Code, f64)> = ranked
            .iter()
            .filter(|(c, _)| !ctx.positions.contains_key(c))
            .copied()
            .collect();

        let mut buy_orders = Vec::new();
        if !buy_candidates.is_empty() {
            // 预期回款为毛额口径：Σ(卖出委托量 × T_exec 日 deal_price 列价格)，
            // 不预估滑点与费用（执行层摩擦对策略不可见）
            let expected_proceeds: f64 = sell_orders
                .iter()
                .map(|o| {
                    let price = ctx
                        .tradable
                        .get(o.stock)
                        .filter(|t| t.deal_price.is_finite() && t.deal_price > 0.0)
                        .map(|t| t.deal_price)
                        .unwrap_or(ctx.positions[&o.stock].price);
                    o.volume.abs() * price
                })
                .sum();
            if let Some(amount) =
                common::equal_weight_amount(ctx.cash + expected_proceeds, buy_candidates.len())
            {
                if amount > 0.0 {
                    for (code, _) in &buy_candidates {
                        let price = ctx.tradable.get(*code).expect("候选必可交易").deal_price;
                        let volume = common::amount_to_volume(amount, price);
                        if volume > 0.0 {
                            buy_orders.push(Order::new(*code, volume, price));
                        }
                    }
                }
            }
        }

        Ok(Decision {
            sell_orders,
            // 已按 score 降序（核减时从尾部丢弃低分）
            buy_orders,
            target_positions: Some(self.top_n),
        })
    }
}
