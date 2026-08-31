//! TopkDropoutStrategy（规范"内置策略"）。
//!
//! 目标持仓：预测分数最高的 `top_n` 只股票。每个有信号可用的交易日调仓一次：
//! 当日无 score 的持仓全部卖出（不占 drop_n 名额），再从其余持仓中卖出 score
//! 最差的 `drop_n` 只，然后按 `n_buy = top_n − 卖出后保留持仓数` 等权买入新股票。
//! 两阶段核减（卖不掉继续占坑）由 Backtest 编排、trait 默认钩子执行。

use std::collections::HashMap;

use crate::error::Result;
use crate::order::{Decision, Order};
use crate::strategy::common;
use crate::strategy::{Strategy, StrategyContext};
use crate::types::Code;

/// TopkDropout 策略。
pub struct TopkDropoutStrategy {
    /// 目标持仓只数（>= 1）
    top_n: usize,
    /// 每个调仓日计划卖出的只数（>= 0）
    drop_n: usize,
    /// 卖出候选是否限定当日可交易股票
    only_tradable: bool,
    /// 是否过滤 ST 股（只限制买入）
    forbid_st: bool,
}

impl TopkDropoutStrategy {
    /// `only_tradable` / `forbid_st` 默认 false，builder 方法覆写。
    ///
    /// # Panics
    /// `top_n < 1` 或 `drop_n` 异常（usize 自然 >= 0）属调用方编程错误。
    pub fn new(top_n: usize, drop_n: usize) -> Self {
        assert!(top_n >= 1, "top_n 须 >= 1，收到: {top_n}");
        if drop_n > top_n {
            log::warn!(
                "drop_n({drop_n}) > top_n({top_n})：每日清空排名内持仓再重建，通常非预期"
            );
        }
        Self {
            top_n,
            drop_n,
            only_tradable: false,
            forbid_st: false,
        }
    }

    pub fn with_only_tradable(mut self, only_tradable: bool) -> Self {
        self.only_tradable = only_tradable;
        self
    }

    pub fn with_forbid_st(mut self, forbid_st: bool) -> Self {
        self.forbid_st = forbid_st;
        self
    }

    pub fn top_n(&self) -> usize {
        self.top_n
    }

    pub fn drop_n(&self) -> usize {
        self.drop_n
    }
}

impl Strategy for TopkDropoutStrategy {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision> {
        let score_of: HashMap<Code, f64> = ctx.signal.as_map();

        // ---- 1. 卖出（method_sell = "bottom"）----
        // 当日无 score 的持仓全部卖出（不占 drop_n 名额）；
        // 其余持仓按 score 升序取最差 drop_n 只卖出
        let mut no_score: Vec<Code> = Vec::new();
        let mut sell_rank: Vec<(Code, f64)> = Vec::new();
        for c in ctx.positions.keys() {
            match score_of.get(c) {
                Some(s) => sell_rank.push((*c, *s)),
                None => no_score.push(*c),
            }
        }
        if self.only_tradable {
            // 不可交易股票不进入卖单、不参与排名，留到下一调仓日
            let sellable = |c: &Code| ctx.tradable.get(*c).is_some_and(|t| t.sellable());
            no_score.retain(sellable);
            sell_rank.retain(|(c, _)| sellable(c));
        }
        // 无 score 组按代码升序保证确定性；有 score 组按 score 升序、同分按代码
        no_score.sort_unstable();
        common::sort_by_score_asc(&mut sell_rank);
        let sell_codes: Vec<Code> = no_score
            .into_iter()
            .chain(sell_rank.iter().take(self.drop_n).map(|(c, _)| *c))
            .collect();
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

        // ---- 2. 买入只数（method_buy = "top"）----
        // n_buy = top_n − 卖出后保留的持仓数；同时不超过当日可用候选数量（take 截断）。
        // 持仓数已不少于 top_n（历史超配）时 saturating_sub 得 0，本日不生成买单。
        let kept = ctx.positions.len() - sell_orders.len();
        let n_buy = self.top_n.saturating_sub(kept);

        // ---- 3. 买入候选与资金分配 ----
        let mut buy_orders = Vec::new();
        if n_buy > 0 {
            let mut candidates: Vec<(Code, f64)> = ctx
                .signal
                .codes
                .iter()
                .copied()
                .zip(ctx.signal.scores.iter().copied())
                // 未持有
                .filter(|(c, _)| !ctx.positions.contains_key(c))
                // 当日可交易（含 deal_price 有效）
                .filter(|(c, _)| ctx.tradable.get(*c).is_some_and(|t| t.buyable()))
                // forbid_st：只限制买入
                .filter(|(c, _)| {
                    !(self.forbid_st && ctx.tradable.get(*c).is_some_and(|t| t.is_st))
                })
                .collect();
            common::sort_by_score_desc(&mut candidates);
            candidates.truncate(n_buy);

            if !candidates.is_empty() {
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
                // 金额一次性确定：后续核减只丢单、不重新分配
                if let Some(amount) =
                    common::equal_weight_amount(ctx.cash + expected_proceeds, candidates.len())
                {
                    if amount > 0.0 {
                        for (code, _) in &candidates {
                            let price = ctx.tradable.get(*code).expect("候选必可交易").deal_price;
                            let volume = common::amount_to_volume(amount, price);
                            if volume > 0.0 {
                                buy_orders.push(Order::new(*code, volume, price));
                            }
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
