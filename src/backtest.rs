//! Backtest（回测执行器，架构 §4.8）：主循环、两阶段撮合编排、延期校验、进度条与耗时。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::account::Account;
use crate::data::benchmark::BenchmarkStore;
use crate::data::calendar::TradingCalendar;
use crate::error::{BtError, Result};
use crate::exchange::Exchange;
use crate::order::{Decision, Order, Side, TradeRecord};
use crate::result::BTResult;
use crate::signal::{Signal, SignalDay};
use crate::strategy::{PostSellContext, Strategy, StrategyContext};
use crate::types::DayIdx;

/// 回测执行器：按交易日历逐步推进（取信号 -> 复权调整 -> 决策 -> 两阶段撮合 -> 估值记账）。
pub struct Backtest {
    calendar: TradingCalendar,
    benchmark: Option<Arc<BenchmarkStore>>,
    account: Account,
    exchange: Exchange,
    strategy: Box<dyn Strategy>,
    /// 账户构造时的期初现金（装配 BTResult 时导出：报表首日分母）
    initial_cash: f64,
    /// 终端进度条开关，默认 false
    progress: bool,
    /// run 单次性守卫：首次运行消耗账户 / 基准 / 逐日记录状态，禁止二次调用
    has_run: bool,
}

impl Backtest {
    /// 装配：exchange 注入行情（按 deal_price 预计算 limit 列）。
    ///
    /// 装配期校验 wap 数据与 deal_price 匹配（vwapN/twapN 必须提供对应时段的 wap
    /// 数据），不匹配返回 `Err`。
    pub fn new(
        data: crate::data::BTData,
        account: Account,
        mut exchange: Exchange,
        strategy: Box<dyn Strategy>,
    ) -> Result<Self> {
        let crate::data::BTData {
            stock_bar,
            benchmark,
            wap,
        } = data;
        let Some(stock_bar) = stock_bar else {
            return Err(BtError::InvalidParam(
                "Backtest::new 需要 stock_bar 数据，请先调用 BTData::load_stock_bar".into(),
            ));
        };
        let calendar = stock_bar.calendar.clone();
        exchange.inject_market(stock_bar, wap)?;
        let initial_cash = account.cash();
        Ok(Self {
            calendar,
            benchmark,
            account,
            exchange,
            strategy,
            initial_cash,
            progress: false,
            has_run: false,
        })
    }

    /// 进度条开关（默认关闭）：启用后 run 期间向 stderr 渲染按交易日推进的进度条，
    /// 结束行显示总耗时。开关与否不改变逐日账户序列与成交记录。
    pub fn with_progress(mut self, enabled: bool) -> Self {
        self.progress = enabled;
        self
    }

    /// 运行回测：区间 [start_date, end_date) 按交易日历对齐。
    ///
    /// 只能调用一次：首次运行消耗账户持仓 / 基准数据 / 逐日记录（`take` 语义），
    /// 二次调用直接报错；如需再次回测请重新装配 `Backtest`。
    pub fn run(&mut self, signal: &Signal, start_date: &str, end_date: &str) -> Result<BTResult> {
        if self.has_run {
            return Err(BtError::InvalidParam(
                "Backtest::run 只能调用一次（账户与基准状态已在首次运行中消耗），请重新装配 Backtest"
                    .into(),
            ));
        }
        let timer = Instant::now();

        // ---- 0. 启动校验 ----

        // ---- 0. 启动校验 ----
        let range = self.calendar.align(start_date, end_date)?;
        // 信号延期校验（依赖交易日历，推迟到 run 启动时执行）：
        // datetime 不在交易日历 -> 丢弃 + warning；instrument 无行情 -> 丢弃 + warning
        let code_set = self.exchange.code_set();
        let mut signals: HashMap<DayIdx, SignalDay> = HashMap::new();
        let mut dropped_date = 0usize;
        let mut dropped_instrument = 0usize;
        for (date, sd) in &signal.days {
            let Some(day) = self.calendar.day_idx(date) else {
                dropped_date += 1;
                continue;
            };
            let mut codes = Vec::with_capacity(sd.codes.len());
            let mut scores = Vec::with_capacity(sd.scores.len());
            for (c, s) in sd.codes.iter().zip(&sd.scores) {
                if code_set.contains(c) {
                    codes.push(*c);
                    scores.push(*s);
                } else {
                    dropped_instrument += 1;
                }
            }
            // 若过滤后当日无有效信号，视同无信号日：不插入 map，
            // 下一交易日不会收到空 SignalDay，避免策略误判为“全市场无 score”而清仓。
            if !codes.is_empty() {
                signals.insert(day, SignalDay { codes, scores });
            }
        }
        if dropped_date > 0 {
            log::warn!("pred: datetime 不在交易日历中，丢弃 {dropped_date} 个信号日");
        }
        if dropped_instrument > 0 {
            log::warn!("pred: instrument 无行情数据，丢弃 {dropped_instrument} 条信号");
        }

        // 启动校验通过，标记已运行（此前报错不影响二次调用）
        self.has_run = true;

        // 进度条：stderr 渲染；总日数 = 对齐区间交易日数（启动校验后即知）
        let n_days = range.len();
        let pb = self.progress.then(|| {
            let bar = ProgressBar::with_draw_target(
                Some(n_days as u64),
                ProgressDrawTarget::stderr(),
            );
            bar.set_style(
                ProgressStyle::with_template(
                    "[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} 日 ETA {eta_precise} {msg}",
                )
                .expect("进度条模板合法"),
            );
            bar
        });

        let mut trades: Vec<TradeRecord> = Vec::new();

        // ---- 1. 主循环 ----
        for day in range.clone() {
            // b. 撮合前复权调整（与当日是否交易无关）
            let view = self.exchange.day_view(day);
            self.account.adjust_factor(&view);

            // a. 取 T−1 日信号（前一交易日）；无信号日本日不产生任何订单，
            //    跳过决策与撮合，仅执行复权与估值
            let prev_signal = day
                .checked_sub(1)
                .and_then(|prev| signals.get(&prev));

            if let Some(sig) = prev_signal {
                // c. 生成决策
                let ctx = StrategyContext {
                    signal: sig,
                    positions: self.account.positions(),
                    cash: self.account.cash(),
                    tradable: view.tradable_info(),
                    day,
                };
                let decision = self.strategy.gen_decision(&ctx)?;
                let decision = normalize_decision(decision)?;

                // d. 阶段一：卖单全部撮合（回款当日可用）
                let mut filled_sells: Vec<Order> = Vec::with_capacity(decision.sell_orders.len());
                for mut order in decision.sell_orders {
                    self.exchange.deal_order(&mut order, &mut self.account, day);
                    trades.push(trade_record(&order, day));
                    filled_sells.push(order);
                }

                // e. 核减：默认按 target_positions 截断买单
                let post_ctx = PostSellContext {
                    positions: self.account.positions(),
                    cash: self.account.cash(),
                    tradable: view.tradable_info(),
                    filled_sells: &filled_sells,
                    target_positions: decision.target_positions,
                };
                let buy_orders = self
                    .strategy
                    .revise_buy_orders(decision.buy_orders, &post_ctx)?;

                // f. 阶段二：按序逐单撮合买单（优先级高者优先获得资金）；
                //    被核减丢弃的买单未进入撮合，不产生 trades 行
                for mut order in buy_orders {
                    self.exchange.deal_order(&mut order, &mut self.account, day);
                    trades.push(trade_record(&order, day));
                }
            }

            // g. 日终估值与记账
            self.account.end_of_day(&view, day);

            // h. 进度推进（禁用时 None，零开销）
            if let Some(bar) = &pb {
                bar.inc(1);
            }
        }

        // ---- 2. 装配 BTResult（calendar 克隆等收尾计入 elapsed）----
        let mut result = BTResult::assemble(
            self.account.take_daily(),
            self.account.take_hist_positions(),
            trades,
            self.calendar.clone(),
            range,
            self.benchmark.take(),
            self.initial_cash,
        );
        let elapsed = timer.elapsed();
        result.set_elapsed(elapsed);
        if let Some(bar) = pb {
            bar.finish_with_message(format!("完成，总耗时 {elapsed:.2?}"));
        }

        Ok(result)
    }

}

/// Decision 合法性规整（规范"撮合通用规则--当日新买入的复核"）：
/// 同股同时买 + 卖 -> Err；同股多笔买单合并为一笔（多笔卖单对称合并）。
fn normalize_decision(mut decision: Decision) -> Result<Decision> {
    fn merge(orders: Vec<Order>) -> Vec<Order> {
        let mut out: Vec<Order> = Vec::with_capacity(orders.len());
        for o in orders {
            match out.iter_mut().find(|x| x.stock == o.stock) {
                Some(x) => x.volume += o.volume,
                None => out.push(o),
            }
        }
        out
    }
    decision.buy_orders = merge(decision.buy_orders);
    decision.sell_orders = merge(decision.sell_orders);
    for buy in &decision.buy_orders {
        if decision.sell_orders.iter().any(|s| s.stock == buy.stock) {
            return Err(BtError::InvalidDecision(format!(
                "同一交易步内买、卖同一股票 code={}",
                buy.stock
            )));
        }
    }
    Ok(decision)
}

/// 订单 -> 成交记录（含未成交订单，deal_volume = 0）。
fn trade_record(order: &Order, day: DayIdx) -> TradeRecord {
    TradeRecord {
        day,
        stock: order.stock,
        side: if order.volume > 0.0 { Side::Buy } else { Side::Sell },
        volume: order.volume.abs(),
        price: order.price,
        deal_volume: order.deal_volume.abs(),
        deal_price: order.deal_price,
        deal_cost: order.deal_cost,
    }
}
